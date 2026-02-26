use anchor_lang::prelude::*;
use anchor_lang::solana_program::bpf_loader_upgradeable;
use anchor_spl::token_interface::{
    close_account, CloseAccount, Mint, TokenAccount, TokenInterface,
};

declare_id!("8CJi79SkfMYN29XX4WmBT8AmtvCrAzzrFYJsdai6oKwL");

/// Cranker fee: 5% of reclaimed rent
const CRANKER_FEE_BPS: u64 = 500;
/// Protocol fee: 5% of reclaimed rent, accumulates in fee vault
const PROTOCOL_FEE_BPS: u64 = 500;
const BPS_DENOMINATOR: u64 = 10_000;

#[program]
pub mod chiefburner {
    use super::*;

    /// Initialize the fee vault PDA. Permissionless, only needs to be called once.
    pub fn initialize(ctx: Context<Initialize>) -> Result<()> {
        ctx.accounts.fee_vault.bump = ctx.bumps.fee_vault;
        Ok(())
    }

    /// Close a token account and split the reclaimed rent:
    /// - 5% to the cranker (tx submitter)
    /// - 5% to the protocol fee vault
    /// - 90% to the account owner
    ///
    /// Permissionless: anyone can submit this transaction as cranker.
    /// The token account must have zero balance.
    pub fn burn_and_close(ctx: Context<BurnAndClose>) -> Result<()> {
        let token_account = &ctx.accounts.token_account;

        // Enforce zero balance — we don't burn dust
        require!(token_account.amount == 0, ChiefburnerError::NonZeroBalance);

        // Snapshot the lamports before closing
        let rent_lamports = token_account.to_account_info().lamports();

        // CPI: close the token account — rent goes to the owner first
        close_account(CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            CloseAccount {
                account: ctx.accounts.token_account.to_account_info(),
                destination: ctx.accounts.owner.to_account_info(),
                authority: ctx.accounts.owner.to_account_info(),
            },
        ))?;

        // Verify the account was actually closed
        require!(
            ctx.accounts.token_account.to_account_info().lamports() == 0,
            ChiefburnerError::CloseDidNotSucceed
        );

        // Calculate fees
        let cranker_fee = rent_lamports
            .checked_mul(CRANKER_FEE_BPS)
            .unwrap()
            .checked_div(BPS_DENOMINATOR)
            .unwrap();

        let protocol_fee = rent_lamports
            .checked_mul(PROTOCOL_FEE_BPS)
            .unwrap()
            .checked_div(BPS_DENOMINATOR)
            .unwrap();

        let owner_info = ctx.accounts.owner.to_account_info();

        // Transfer cranker fee from owner to cranker
        if cranker_fee > 0 && ctx.accounts.cranker.key() != ctx.accounts.owner.key() {
            let cranker_info = ctx.accounts.cranker.to_account_info();
            **owner_info.try_borrow_mut_lamports()? -= cranker_fee;
            **cranker_info.try_borrow_mut_lamports()? += cranker_fee;
        }

        // Transfer protocol fee from owner to fee vault
        if protocol_fee > 0 {
            let fee_vault_info = ctx.accounts.fee_vault.to_account_info();
            **owner_info.try_borrow_mut_lamports()? -= protocol_fee;
            **fee_vault_info.try_borrow_mut_lamports()? += protocol_fee;
        }

        emit!(AccountClosed {
            token_account: ctx.accounts.token_account.key(),
            mint: ctx.accounts.mint.key(),
            owner: ctx.accounts.owner.key(),
            cranker: ctx.accounts.cranker.key(),
            rent_reclaimed: rent_lamports,
            cranker_fee,
            protocol_fee,
        });

        Ok(())
    }

    /// Withdraw accumulated protocol fees to the program upgrade authority.
    pub fn collect_fees(ctx: Context<CollectFees>) -> Result<()> {
        let fee_vault_info = ctx.accounts.fee_vault.to_account_info();
        let rent = Rent::get()?;
        let min_balance = rent.minimum_balance(FeeVault::INIT_SPACE + 8);
        let excess = fee_vault_info
            .lamports()
            .checked_sub(min_balance)
            .unwrap_or(0);

        require!(excess > 0, ChiefburnerError::NoFeesToCollect);

        **fee_vault_info.try_borrow_mut_lamports()? -= excess;
        **ctx
            .accounts
            .authority
            .to_account_info()
            .try_borrow_mut_lamports()? += excess;

        emit!(FeesCollected {
            authority: ctx.accounts.authority.key(),
            amount: excess,
        });

        Ok(())
    }
}

#[derive(Accounts)]
pub struct Initialize<'info> {
    /// The fee vault PDA — holds accumulated protocol fees.
    #[account(
        init,
        payer = payer,
        space = 8 + FeeVault::INIT_SPACE,
        seeds = [b"fee_vault"],
        bump,
    )]
    pub fee_vault: Account<'info, FeeVault>,

    #[account(mut)]
    pub payer: Signer<'info>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct BurnAndClose<'info> {
    /// The token account to close. Must have zero balance.
    #[account(
        mut,
        token::mint = mint,
        token::authority = owner,
    )]
    pub token_account: InterfaceAccount<'info, TokenAccount>,

    /// The mint of the token account.
    pub mint: InterfaceAccount<'info, Mint>,

    /// The owner of the token account. Must sign to authorize closing.
    #[account(mut)]
    pub owner: Signer<'info>,

    /// The cranker submitting the transaction. Receives 5% of reclaimed rent.
    /// CHECK: Any account can be the cranker — this is permissionless.
    #[account(mut)]
    pub cranker: UncheckedAccount<'info>,

    /// The fee vault PDA — accumulates protocol fees.
    #[account(
        mut,
        seeds = [b"fee_vault"],
        bump = fee_vault.bump,
    )]
    pub fee_vault: Account<'info, FeeVault>,

    /// The token program (supports both Token and Token-2022).
    pub token_program: Interface<'info, TokenInterface>,
}

#[derive(Accounts)]
pub struct CollectFees<'info> {
    /// The fee vault PDA holding accumulated fees.
    #[account(
        mut,
        seeds = [b"fee_vault"],
        bump = fee_vault.bump,
    )]
    pub fee_vault: Account<'info, FeeVault>,

    /// Must be the program's upgrade authority.
    #[account(mut)]
    pub authority: Signer<'info>,

    /// The program's programdata account — used to verify upgrade authority.
    #[account(
        seeds = [crate::ID.as_ref()],
        bump,
        seeds::program = bpf_loader_upgradeable::ID,
        constraint = program_data.upgrade_authority_address == Some(authority.key()) @ ChiefburnerError::Unauthorized,
    )]
    pub program_data: Account<'info, ProgramData>,
}

#[account]
#[derive(InitSpace)]
pub struct FeeVault {
    pub bump: u8,
}

#[event]
pub struct AccountClosed {
    pub token_account: Pubkey,
    pub mint: Pubkey,
    pub owner: Pubkey,
    pub cranker: Pubkey,
    pub rent_reclaimed: u64,
    pub cranker_fee: u64,
    pub protocol_fee: u64,
}

#[event]
pub struct FeesCollected {
    pub authority: Pubkey,
    pub amount: u64,
}

#[error_code]
pub enum ChiefburnerError {
    #[msg("Token account balance must be zero to close")]
    NonZeroBalance,
    #[msg("Not authorized — must be program upgrade authority")]
    Unauthorized,
    #[msg("No fees available to collect")]
    NoFeesToCollect,
    #[msg("Token account close did not succeed")]
    CloseDidNotSucceed,
}
