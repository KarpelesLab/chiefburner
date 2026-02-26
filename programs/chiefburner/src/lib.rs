use anchor_lang::prelude::*;
use anchor_spl::token_interface::{
    close_account, CloseAccount, Mint, TokenAccount, TokenInterface,
};

declare_id!("8CJi79SkfMYN29XX4WmBT8AmtvCrAzzrFYJsdai6oKwL");

/// Cranker fee: 5% of reclaimed rent
const CRANKER_FEE_BPS: u64 = 500;
const BPS_DENOMINATOR: u64 = 10_000;

#[program]
pub mod chiefburner {
    use super::*;

    /// Close a token account and split the reclaimed rent between
    /// the cranker (5%) and the account owner (95%).
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

        // Calculate cranker fee
        let cranker_fee = rent_lamports
            .checked_mul(CRANKER_FEE_BPS)
            .unwrap()
            .checked_div(BPS_DENOMINATOR)
            .unwrap();

        // Transfer cranker fee from owner to cranker
        if cranker_fee > 0 && ctx.accounts.cranker.key() != ctx.accounts.owner.key() {
            let owner_info = ctx.accounts.owner.to_account_info();
            let cranker_info = ctx.accounts.cranker.to_account_info();

            **owner_info.try_borrow_mut_lamports()? -= cranker_fee;
            **cranker_info.try_borrow_mut_lamports()? += cranker_fee;
        }

        emit!(AccountClosed {
            token_account: ctx.accounts.token_account.key(),
            mint: ctx.accounts.mint.key(),
            owner: ctx.accounts.owner.key(),
            cranker: ctx.accounts.cranker.key(),
            rent_reclaimed: rent_lamports,
            cranker_fee,
        });

        Ok(())
    }
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

    /// The token program (supports both Token and Token-2022).
    pub token_program: Interface<'info, TokenInterface>,
}

#[event]
pub struct AccountClosed {
    pub token_account: Pubkey,
    pub mint: Pubkey,
    pub owner: Pubkey,
    pub cranker: Pubkey,
    pub rent_reclaimed: u64,
    pub cranker_fee: u64,
}

#[error_code]
pub enum ChiefburnerError {
    #[msg("Token account balance must be zero to close")]
    NonZeroBalance,
    #[msg("Token account close did not succeed")]
    CloseDidNotSucceed,
}
