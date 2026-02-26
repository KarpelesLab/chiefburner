use anchor_lang::prelude::*;
use anchor_lang::solana_program::{instruction::Instruction, program::invoke};
use anchor_spl::token_interface::{
    burn, close_account, Burn, CloseAccount, Mint, TokenAccount, TokenInterface,
};

declare_id!("8CJi79SkfMYN29XX4WmBT8AmtvCrAzzrFYJsdai6oKwL");

/// Cranker fee: 5% of reclaimed rent
const CRANKER_FEE_BPS: u64 = 500;
const BPS_DENOMINATOR: u64 = 10_000;

/// Metaplex Token Metadata program
const METAPLEX_METADATA_PROGRAM: Pubkey =
    pubkey!("metaqbxxUerdq28cj1RbAWkYQm3ybzjb6a8bt518x1s");

/// SPL Name Service program
const NAME_SERVICE_PROGRAM: Pubkey =
    pubkey!("namesLPneVptA9Z5rqUDD9tMTWEJwofgaYwp8cawRkX");

/// Transfer cranker fee (5%) from owner to cranker. Returns the fee amount.
fn transfer_cranker_fee<'info>(
    owner: &AccountInfo<'info>,
    cranker: &AccountInfo<'info>,
    rent_lamports: u64,
) -> Result<u64> {
    let cranker_fee = rent_lamports
        .checked_mul(CRANKER_FEE_BPS)
        .unwrap()
        .checked_div(BPS_DENOMINATOR)
        .unwrap();

    if cranker_fee > 0 && cranker.key() != owner.key() {
        **owner.try_borrow_mut_lamports()? -= cranker_fee;
        **cranker.try_borrow_mut_lamports()? += cranker_fee;
    }

    Ok(cranker_fee)
}

#[program]
pub mod chiefburner {
    use super::*;

    // ── Token instructions ──────────────────────────────────────────

    /// Burn any remaining tokens and close the account.
    /// Splits reclaimed rent: 5% to cranker, 95% to owner.
    pub fn burn_and_close(ctx: Context<BurnAndClose>) -> Result<()> {
        let token_account = &ctx.accounts.token_account;
        let rent_lamports = token_account.to_account_info().lamports();

        // Burn tokens if non-zero balance
        if token_account.amount > 0 {
            burn(
                CpiContext::new(
                    ctx.accounts.token_program.to_account_info(),
                    Burn {
                        mint: ctx.accounts.mint.to_account_info(),
                        from: ctx.accounts.token_account.to_account_info(),
                        authority: ctx.accounts.owner.to_account_info(),
                    },
                ),
                token_account.amount,
            )?;
        }

        // Close the token account — rent goes to owner
        close_account(CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            CloseAccount {
                account: ctx.accounts.token_account.to_account_info(),
                destination: ctx.accounts.owner.to_account_info(),
                authority: ctx.accounts.owner.to_account_info(),
            },
        ))?;

        require!(
            ctx.accounts.token_account.to_account_info().lamports() == 0,
            ChiefburnerError::CloseDidNotSucceed
        );

        let cranker_fee = transfer_cranker_fee(
            &ctx.accounts.owner.to_account_info(),
            &ctx.accounts.cranker,
            rent_lamports,
        )?;

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

    /// Burn any remaining tokens and close the account.
    /// All reclaimed rent goes to the cranker.
    pub fn burn_and_close_free(ctx: Context<BurnAndCloseFree>) -> Result<()> {
        let token_account = &ctx.accounts.token_account;
        let rent_lamports = token_account.to_account_info().lamports();

        if token_account.amount > 0 {
            burn(
                CpiContext::new(
                    ctx.accounts.token_program.to_account_info(),
                    Burn {
                        mint: ctx.accounts.mint.to_account_info(),
                        from: ctx.accounts.token_account.to_account_info(),
                        authority: ctx.accounts.owner.to_account_info(),
                    },
                ),
                token_account.amount,
            )?;
        }

        // Close — rent goes directly to cranker
        close_account(CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            CloseAccount {
                account: ctx.accounts.token_account.to_account_info(),
                destination: ctx.accounts.cranker.to_account_info(),
                authority: ctx.accounts.owner.to_account_info(),
            },
        ))?;

        require!(
            ctx.accounts.token_account.to_account_info().lamports() == 0,
            ChiefburnerError::CloseDidNotSucceed
        );

        emit!(AccountClosed {
            token_account: ctx.accounts.token_account.key(),
            mint: ctx.accounts.mint.key(),
            owner: ctx.accounts.owner.key(),
            cranker: ctx.accounts.cranker.key(),
            rent_reclaimed: rent_lamports,
            cranker_fee: rent_lamports,
        });

        Ok(())
    }

    // ── NFT instructions (Metaplex Token Metadata) ──────────────────

    /// Burn a regular NFT via Metaplex and split recovered rent (~0.015 SOL).
    /// 5% to cranker, 95% to owner.
    pub fn burn_nft(ctx: Context<BurnNft>) -> Result<()> {
        let owner_info = ctx.accounts.owner.to_account_info();
        let lamports_before = owner_info.lamports();

        // CPI: Metaplex BurnNft (instruction discriminator = 18)
        let ix = Instruction {
            program_id: METAPLEX_METADATA_PROGRAM,
            accounts: vec![
                AccountMeta::new(ctx.accounts.metadata.key(), false),
                AccountMeta::new(ctx.accounts.owner.key(), true),
                AccountMeta::new(ctx.accounts.mint.key(), false),
                AccountMeta::new(ctx.accounts.token_account.key(), false),
                AccountMeta::new(ctx.accounts.master_edition.key(), false),
                AccountMeta::new_readonly(ctx.accounts.token_program.key(), false),
            ],
            data: vec![18], // BurnNft discriminator
        };

        invoke(
            &ix,
            &[
                ctx.accounts.metadata.to_account_info(),
                owner_info.clone(),
                ctx.accounts.mint.to_account_info(),
                ctx.accounts.token_account.to_account_info(),
                ctx.accounts.master_edition.to_account_info(),
                ctx.accounts.token_program.to_account_info(),
            ],
        )?;

        let rent_reclaimed = owner_info
            .lamports()
            .checked_sub(lamports_before)
            .unwrap_or(0);

        let cranker_fee = transfer_cranker_fee(
            &owner_info,
            &ctx.accounts.cranker,
            rent_reclaimed,
        )?;

        emit!(NftBurned {
            owner: ctx.accounts.owner.key(),
            mint: ctx.accounts.mint.key(),
            cranker: ctx.accounts.cranker.key(),
            rent_reclaimed,
            cranker_fee,
        });

        Ok(())
    }

    /// Burn a regular NFT via Metaplex. All recovered rent to cranker.
    pub fn burn_nft_free(ctx: Context<BurnNftFree>) -> Result<()> {
        // Rent goes to cranker by passing cranker as the "owner" destination
        // But Metaplex sends rent to the NFT owner, so we close into owner then transfer all
        let owner_info = ctx.accounts.owner.to_account_info();
        let lamports_before = owner_info.lamports();

        let ix = Instruction {
            program_id: METAPLEX_METADATA_PROGRAM,
            accounts: vec![
                AccountMeta::new(ctx.accounts.metadata.key(), false),
                AccountMeta::new(ctx.accounts.owner.key(), true),
                AccountMeta::new(ctx.accounts.mint.key(), false),
                AccountMeta::new(ctx.accounts.token_account.key(), false),
                AccountMeta::new(ctx.accounts.master_edition.key(), false),
                AccountMeta::new_readonly(ctx.accounts.token_program.key(), false),
            ],
            data: vec![18],
        };

        invoke(
            &ix,
            &[
                ctx.accounts.metadata.to_account_info(),
                owner_info.clone(),
                ctx.accounts.mint.to_account_info(),
                ctx.accounts.token_account.to_account_info(),
                ctx.accounts.master_edition.to_account_info(),
                ctx.accounts.token_program.to_account_info(),
            ],
        )?;

        let rent_reclaimed = owner_info
            .lamports()
            .checked_sub(lamports_before)
            .unwrap_or(0);

        if rent_reclaimed > 0 {
            let cranker_info = ctx.accounts.cranker.to_account_info();
            **owner_info.try_borrow_mut_lamports()? -= rent_reclaimed;
            **cranker_info.try_borrow_mut_lamports()? += rent_reclaimed;
        }

        emit!(NftBurned {
            owner: ctx.accounts.owner.key(),
            mint: ctx.accounts.mint.key(),
            cranker: ctx.accounts.cranker.key(),
            rent_reclaimed,
            cranker_fee: rent_reclaimed,
        });

        Ok(())
    }

    // ── Domain instructions (SPL Name Service) ──────────────────────

    /// Delete a .sol domain and split recovered rent (~0.003 SOL).
    /// 5% to cranker, 95% to owner.
    pub fn delete_domain(ctx: Context<DeleteDomain>) -> Result<()> {
        let owner_info = ctx.accounts.owner.to_account_info();
        let lamports_before = owner_info.lamports();

        // CPI: Name Service Delete (instruction discriminator = 3)
        let ix = Instruction {
            program_id: NAME_SERVICE_PROGRAM,
            accounts: vec![
                AccountMeta::new(ctx.accounts.name_account.key(), false),
                AccountMeta::new(ctx.accounts.owner.key(), true),
                AccountMeta::new(ctx.accounts.refund_target.key(), false),
            ],
            data: vec![3], // Delete discriminator
        };

        invoke(
            &ix,
            &[
                ctx.accounts.name_account.to_account_info(),
                owner_info.clone(),
                ctx.accounts.refund_target.to_account_info(),
            ],
        )?;

        let rent_reclaimed = ctx
            .accounts
            .refund_target
            .to_account_info()
            .lamports()
            .checked_sub(lamports_before)
            .unwrap_or(0);

        let cranker_fee = transfer_cranker_fee(
            &ctx.accounts.refund_target.to_account_info(),
            &ctx.accounts.cranker,
            rent_reclaimed,
        )?;

        emit!(DomainDeleted {
            owner: ctx.accounts.owner.key(),
            name_account: ctx.accounts.name_account.key(),
            cranker: ctx.accounts.cranker.key(),
            rent_reclaimed,
            cranker_fee,
        });

        Ok(())
    }

    /// Delete a .sol domain. All recovered rent goes to cranker.
    pub fn delete_domain_free(ctx: Context<DeleteDomainFree>) -> Result<()> {
        // Name Service refunds to a target account — send directly to cranker
        let ix = Instruction {
            program_id: NAME_SERVICE_PROGRAM,
            accounts: vec![
                AccountMeta::new(ctx.accounts.name_account.key(), false),
                AccountMeta::new(ctx.accounts.owner.key(), true),
                AccountMeta::new(ctx.accounts.cranker.key(), false),
            ],
            data: vec![3],
        };

        let name_lamports = ctx.accounts.name_account.to_account_info().lamports();

        invoke(
            &ix,
            &[
                ctx.accounts.name_account.to_account_info(),
                ctx.accounts.owner.to_account_info(),
                ctx.accounts.cranker.to_account_info(),
            ],
        )?;

        emit!(DomainDeleted {
            owner: ctx.accounts.owner.key(),
            name_account: ctx.accounts.name_account.key(),
            cranker: ctx.accounts.cranker.key(),
            rent_reclaimed: name_lamports,
            cranker_fee: name_lamports,
        });

        Ok(())
    }
}

// ── Account structs ─────────────────────────────────────────────────

#[derive(Accounts)]
pub struct BurnAndClose<'info> {
    /// The token account to close.
    #[account(
        mut,
        token::mint = mint,
        token::authority = owner,
    )]
    pub token_account: InterfaceAccount<'info, TokenAccount>,

    /// The mint of the token account.
    #[account(mut)]
    pub mint: InterfaceAccount<'info, Mint>,

    /// The owner of the token account. Must sign to authorize.
    #[account(mut)]
    pub owner: Signer<'info>,

    /// The cranker. Receives 5% of reclaimed rent.
    /// CHECK: Any account can be the cranker.
    #[account(mut)]
    pub cranker: UncheckedAccount<'info>,

    /// The token program (supports both Token and Token-2022).
    pub token_program: Interface<'info, TokenInterface>,
}

#[derive(Accounts)]
pub struct BurnAndCloseFree<'info> {
    /// The token account to close.
    #[account(
        mut,
        token::mint = mint,
        token::authority = owner,
    )]
    pub token_account: InterfaceAccount<'info, TokenAccount>,

    /// The mint of the token account.
    #[account(mut)]
    pub mint: InterfaceAccount<'info, Mint>,

    /// The owner of the token account. Must sign to authorize.
    pub owner: Signer<'info>,

    /// The cranker. Receives 100% of reclaimed rent.
    /// CHECK: Any account can be the cranker.
    #[account(mut)]
    pub cranker: UncheckedAccount<'info>,

    /// The token program (supports both Token and Token-2022).
    pub token_program: Interface<'info, TokenInterface>,
}

#[derive(Accounts)]
pub struct BurnNft<'info> {
    /// The NFT owner. Must sign.
    #[account(mut)]
    pub owner: Signer<'info>,

    /// The cranker. Receives 5% of recovered rent.
    /// CHECK: Any account can be the cranker.
    #[account(mut)]
    pub cranker: UncheckedAccount<'info>,

    /// The NFT mint.
    /// CHECK: Validated by Metaplex CPI.
    #[account(mut)]
    pub mint: UncheckedAccount<'info>,

    /// The token account holding the NFT.
    /// CHECK: Validated by Metaplex CPI.
    #[account(mut)]
    pub token_account: UncheckedAccount<'info>,

    /// The metadata PDA.
    /// CHECK: Validated by Metaplex CPI.
    #[account(mut)]
    pub metadata: UncheckedAccount<'info>,

    /// The master edition PDA.
    /// CHECK: Validated by Metaplex CPI.
    #[account(mut)]
    pub master_edition: UncheckedAccount<'info>,

    /// SPL Token program.
    /// CHECK: Validated by Metaplex CPI.
    pub token_program: UncheckedAccount<'info>,

    /// Metaplex Token Metadata program.
    /// CHECK: Must be the Metaplex program.
    #[account(address = METAPLEX_METADATA_PROGRAM)]
    pub metadata_program: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct BurnNftFree<'info> {
    /// The NFT owner. Must sign.
    #[account(mut)]
    pub owner: Signer<'info>,

    /// The cranker. Receives 100% of recovered rent.
    /// CHECK: Any account can be the cranker.
    #[account(mut)]
    pub cranker: UncheckedAccount<'info>,

    /// The NFT mint.
    /// CHECK: Validated by Metaplex CPI.
    #[account(mut)]
    pub mint: UncheckedAccount<'info>,

    /// The token account holding the NFT.
    /// CHECK: Validated by Metaplex CPI.
    #[account(mut)]
    pub token_account: UncheckedAccount<'info>,

    /// The metadata PDA.
    /// CHECK: Validated by Metaplex CPI.
    #[account(mut)]
    pub metadata: UncheckedAccount<'info>,

    /// The master edition PDA.
    /// CHECK: Validated by Metaplex CPI.
    #[account(mut)]
    pub master_edition: UncheckedAccount<'info>,

    /// SPL Token program.
    /// CHECK: Validated by Metaplex CPI.
    pub token_program: UncheckedAccount<'info>,

    /// Metaplex Token Metadata program.
    /// CHECK: Must be the Metaplex program.
    #[account(address = METAPLEX_METADATA_PROGRAM)]
    pub metadata_program: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct DeleteDomain<'info> {
    /// The domain owner. Must sign.
    #[account(mut)]
    pub owner: Signer<'info>,

    /// The cranker. Receives 5% of recovered rent.
    /// CHECK: Any account can be the cranker.
    #[account(mut)]
    pub cranker: UncheckedAccount<'info>,

    /// The name account (domain) to delete.
    /// CHECK: Validated by Name Service CPI.
    #[account(mut)]
    pub name_account: UncheckedAccount<'info>,

    /// The account to receive the refund (owner).
    /// CHECK: Receives rent refund from Name Service.
    #[account(mut, address = owner.key())]
    pub refund_target: UncheckedAccount<'info>,

    /// SPL Name Service program.
    /// CHECK: Must be the Name Service program.
    #[account(address = NAME_SERVICE_PROGRAM)]
    pub name_service_program: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct DeleteDomainFree<'info> {
    /// The domain owner. Must sign.
    #[account(mut)]
    pub owner: Signer<'info>,

    /// The cranker. Receives 100% of recovered rent.
    /// CHECK: Any account can be the cranker.
    #[account(mut)]
    pub cranker: UncheckedAccount<'info>,

    /// The name account (domain) to delete.
    /// CHECK: Validated by Name Service CPI.
    #[account(mut)]
    pub name_account: UncheckedAccount<'info>,

    /// SPL Name Service program.
    /// CHECK: Must be the Name Service program.
    #[account(address = NAME_SERVICE_PROGRAM)]
    pub name_service_program: UncheckedAccount<'info>,
}

// ── Events ──────────────────────────────────────────────────────────

#[event]
pub struct AccountClosed {
    pub token_account: Pubkey,
    pub mint: Pubkey,
    pub owner: Pubkey,
    pub cranker: Pubkey,
    pub rent_reclaimed: u64,
    pub cranker_fee: u64,
}

#[event]
pub struct NftBurned {
    pub owner: Pubkey,
    pub mint: Pubkey,
    pub cranker: Pubkey,
    pub rent_reclaimed: u64,
    pub cranker_fee: u64,
}

#[event]
pub struct DomainDeleted {
    pub owner: Pubkey,
    pub name_account: Pubkey,
    pub cranker: Pubkey,
    pub rent_reclaimed: u64,
    pub cranker_fee: u64,
}

// ── Errors ──────────────────────────────────────────────────────────

#[error_code]
pub enum ChiefburnerError {
    #[msg("Token account close did not succeed")]
    CloseDidNotSucceed,
}
