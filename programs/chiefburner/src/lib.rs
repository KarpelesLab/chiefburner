use anchor_lang::prelude::*;
use anchor_lang::solana_program::{instruction::Instruction, program::invoke};
use anchor_spl::token_interface::{
    burn, close_account, Burn, CloseAccount, TokenInterface,
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

/// Parse mint, owner, and amount from an SPL token account's raw data.
/// Layout is identical for both SPL Token and Token-2022.
fn parse_token_account(data: &[u8]) -> Result<(Pubkey, Pubkey, u64)> {
    require!(data.len() >= 72, ChiefburnerError::InvalidAccount);
    let mint = Pubkey::try_from(&data[0..32]).unwrap();
    let owner = Pubkey::try_from(&data[32..64]).unwrap();
    let amount = u64::from_le_bytes(data[64..72].try_into().unwrap());
    Ok((mint, owner, amount))
}

/// Transfer cranker fee (5%) from source to cranker. Returns the fee amount.
fn transfer_cranker_fee<'info>(
    source: &AccountInfo<'info>,
    cranker: &AccountInfo<'info>,
    rent_lamports: u64,
) -> Result<u64> {
    let cranker_fee = rent_lamports
        .checked_mul(CRANKER_FEE_BPS)
        .unwrap()
        .checked_div(BPS_DENOMINATOR)
        .unwrap();

    if cranker_fee > 0 && cranker.key() != source.key() {
        **source.try_borrow_mut_lamports()? -= cranker_fee;
        **cranker.try_borrow_mut_lamports()? += cranker_fee;
    }

    Ok(cranker_fee)
}

#[program]
pub mod chiefburner {
    use super::*;

    // ── Token instructions ──────────────────────────────────────────
    //
    // remaining_accounts: pairs of (token_account, mint) to burn+close.
    // All token accounts must belong to the owner and the same token program.

    /// Batch burn tokens and close accounts.
    /// Splits reclaimed rent: 5% to cranker, 95% to owner.
    pub fn burn_and_close(ctx: Context<BurnAndClose>) -> Result<()> {
        let remaining = &ctx.remaining_accounts;
        require!(
            remaining.len() >= 2 && remaining.len() % 2 == 0,
            ChiefburnerError::InvalidBatch
        );

        let owner_key = ctx.accounts.owner.key();
        let token_program = &ctx.accounts.token_program;
        let token_program_id = token_program.key();
        let mut total_rent: u64 = 0;

        for chunk in remaining.chunks(2) {
            let token_info = &chunk[0];
            let mint_info = &chunk[1];

            require!(
                *token_info.owner == token_program_id,
                ChiefburnerError::InvalidAccount
            );

            let (mint_key, authority, amount) =
                parse_token_account(&token_info.try_borrow_data()?)?;
            require!(authority == owner_key, ChiefburnerError::InvalidAccount);
            require!(mint_key == mint_info.key(), ChiefburnerError::InvalidAccount);

            let rent = token_info.lamports();
            total_rent = total_rent.checked_add(rent).unwrap();

            if amount > 0 {
                burn(
                    CpiContext::new(
                        token_program.to_account_info(),
                        Burn {
                            mint: mint_info.to_account_info(),
                            from: token_info.to_account_info(),
                            authority: ctx.accounts.owner.to_account_info(),
                        },
                    ),
                    amount,
                )?;
            }

            close_account(CpiContext::new(
                token_program.to_account_info(),
                CloseAccount {
                    account: token_info.to_account_info(),
                    destination: ctx.accounts.owner.to_account_info(),
                    authority: ctx.accounts.owner.to_account_info(),
                },
            ))?;
        }

        let cranker_fee = transfer_cranker_fee(
            &ctx.accounts.owner.to_account_info(),
            &ctx.accounts.cranker,
            total_rent,
        )?;

        emit!(TokensBurned {
            owner: owner_key,
            cranker: ctx.accounts.cranker.key(),
            count: (remaining.len() / 2) as u32,
            total_rent_reclaimed: total_rent,
            cranker_fee,
        });

        Ok(())
    }

    /// Batch burn tokens and close accounts.
    /// All reclaimed rent goes to the cranker.
    pub fn burn_and_close_free(ctx: Context<BurnAndCloseFree>) -> Result<()> {
        let remaining = &ctx.remaining_accounts;
        require!(
            remaining.len() >= 2 && remaining.len() % 2 == 0,
            ChiefburnerError::InvalidBatch
        );

        let owner_key = ctx.accounts.owner.key();
        let token_program = &ctx.accounts.token_program;
        let token_program_id = token_program.key();
        let mut total_rent: u64 = 0;

        for chunk in remaining.chunks(2) {
            let token_info = &chunk[0];
            let mint_info = &chunk[1];

            require!(
                *token_info.owner == token_program_id,
                ChiefburnerError::InvalidAccount
            );

            let (mint_key, authority, amount) =
                parse_token_account(&token_info.try_borrow_data()?)?;
            require!(authority == owner_key, ChiefburnerError::InvalidAccount);
            require!(mint_key == mint_info.key(), ChiefburnerError::InvalidAccount);

            let rent = token_info.lamports();
            total_rent = total_rent.checked_add(rent).unwrap();

            if amount > 0 {
                burn(
                    CpiContext::new(
                        token_program.to_account_info(),
                        Burn {
                            mint: mint_info.to_account_info(),
                            from: token_info.to_account_info(),
                            authority: ctx.accounts.owner.to_account_info(),
                        },
                    ),
                    amount,
                )?;
            }

            close_account(CpiContext::new(
                token_program.to_account_info(),
                CloseAccount {
                    account: token_info.to_account_info(),
                    destination: ctx.accounts.cranker.to_account_info(),
                    authority: ctx.accounts.owner.to_account_info(),
                },
            ))?;
        }

        emit!(TokensBurned {
            owner: owner_key,
            cranker: ctx.accounts.cranker.key(),
            count: (remaining.len() / 2) as u32,
            total_rent_reclaimed: total_rent,
            cranker_fee: total_rent,
        });

        Ok(())
    }

    // ── NFT instructions (Metaplex Token Metadata) ──────────────────
    //
    // remaining_accounts: groups of 4 per NFT:
    //   (metadata, mint, token_account, master_edition)

    /// Batch burn NFTs via Metaplex. 5% to cranker, 95% to owner.
    pub fn burn_nft(ctx: Context<BurnNft>) -> Result<()> {
        let remaining = &ctx.remaining_accounts;
        require!(
            remaining.len() >= 4 && remaining.len() % 4 == 0,
            ChiefburnerError::InvalidBatch
        );

        let owner_info = ctx.accounts.owner.to_account_info();
        let lamports_before = owner_info.lamports();

        for chunk in remaining.chunks(4) {
            let metadata = &chunk[0];
            let mint = &chunk[1];
            let token_account = &chunk[2];
            let master_edition = &chunk[3];

            let ix = Instruction {
                program_id: METAPLEX_METADATA_PROGRAM,
                accounts: vec![
                    AccountMeta::new(metadata.key(), false),
                    AccountMeta::new(ctx.accounts.owner.key(), true),
                    AccountMeta::new(mint.key(), false),
                    AccountMeta::new(token_account.key(), false),
                    AccountMeta::new(master_edition.key(), false),
                    AccountMeta::new_readonly(ctx.accounts.token_program.key(), false),
                ],
                data: vec![18],
            };

            invoke(
                &ix,
                &[
                    metadata.to_account_info(),
                    owner_info.clone(),
                    mint.to_account_info(),
                    token_account.to_account_info(),
                    master_edition.to_account_info(),
                    ctx.accounts.token_program.to_account_info(),
                ],
            )?;
        }

        let rent_reclaimed = owner_info
            .lamports()
            .checked_sub(lamports_before)
            .unwrap_or(0);

        let cranker_fee = transfer_cranker_fee(
            &owner_info,
            &ctx.accounts.cranker,
            rent_reclaimed,
        )?;

        emit!(NftsBurned {
            owner: ctx.accounts.owner.key(),
            cranker: ctx.accounts.cranker.key(),
            count: (remaining.len() / 4) as u32,
            total_rent_reclaimed: rent_reclaimed,
            cranker_fee,
        });

        Ok(())
    }

    /// Batch burn NFTs via Metaplex. All recovered rent to cranker.
    pub fn burn_nft_free(ctx: Context<BurnNft>) -> Result<()> {
        let remaining = &ctx.remaining_accounts;
        require!(
            remaining.len() >= 4 && remaining.len() % 4 == 0,
            ChiefburnerError::InvalidBatch
        );

        let owner_info = ctx.accounts.owner.to_account_info();
        let lamports_before = owner_info.lamports();

        for chunk in remaining.chunks(4) {
            let metadata = &chunk[0];
            let mint = &chunk[1];
            let token_account = &chunk[2];
            let master_edition = &chunk[3];

            let ix = Instruction {
                program_id: METAPLEX_METADATA_PROGRAM,
                accounts: vec![
                    AccountMeta::new(metadata.key(), false),
                    AccountMeta::new(ctx.accounts.owner.key(), true),
                    AccountMeta::new(mint.key(), false),
                    AccountMeta::new(token_account.key(), false),
                    AccountMeta::new(master_edition.key(), false),
                    AccountMeta::new_readonly(ctx.accounts.token_program.key(), false),
                ],
                data: vec![18],
            };

            invoke(
                &ix,
                &[
                    metadata.to_account_info(),
                    owner_info.clone(),
                    mint.to_account_info(),
                    token_account.to_account_info(),
                    master_edition.to_account_info(),
                    ctx.accounts.token_program.to_account_info(),
                ],
            )?;
        }

        let rent_reclaimed = owner_info
            .lamports()
            .checked_sub(lamports_before)
            .unwrap_or(0);

        if rent_reclaimed > 0 {
            let cranker_info = ctx.accounts.cranker.to_account_info();
            **owner_info.try_borrow_mut_lamports()? -= rent_reclaimed;
            **cranker_info.try_borrow_mut_lamports()? += rent_reclaimed;
        }

        emit!(NftsBurned {
            owner: ctx.accounts.owner.key(),
            cranker: ctx.accounts.cranker.key(),
            count: (remaining.len() / 4) as u32,
            total_rent_reclaimed: rent_reclaimed,
            cranker_fee: rent_reclaimed,
        });

        Ok(())
    }

    // ── Domain instructions (SPL Name Service) ──────────────────────
    //
    // remaining_accounts: name_accounts to delete (one per entry).

    /// Batch delete .sol domains. 5% to cranker, 95% to owner.
    pub fn delete_domain(ctx: Context<DeleteDomain>) -> Result<()> {
        let remaining = &ctx.remaining_accounts;
        require!(!remaining.is_empty(), ChiefburnerError::InvalidBatch);

        let owner_info = ctx.accounts.owner.to_account_info();
        let lamports_before = owner_info.lamports();

        for name_account in remaining.iter() {
            let ix = Instruction {
                program_id: NAME_SERVICE_PROGRAM,
                accounts: vec![
                    AccountMeta::new(name_account.key(), false),
                    AccountMeta::new(ctx.accounts.owner.key(), true),
                    AccountMeta::new(ctx.accounts.owner.key(), false),
                ],
                data: vec![3],
            };

            invoke(
                &ix,
                &[
                    name_account.to_account_info(),
                    owner_info.clone(),
                ],
            )?;
        }

        let rent_reclaimed = owner_info
            .lamports()
            .checked_sub(lamports_before)
            .unwrap_or(0);

        let cranker_fee = transfer_cranker_fee(
            &owner_info,
            &ctx.accounts.cranker,
            rent_reclaimed,
        )?;

        emit!(DomainsDeleted {
            owner: ctx.accounts.owner.key(),
            cranker: ctx.accounts.cranker.key(),
            count: remaining.len() as u32,
            total_rent_reclaimed: rent_reclaimed,
            cranker_fee,
        });

        Ok(())
    }

    /// Batch delete .sol domains. All recovered rent goes to cranker.
    pub fn delete_domain_free(ctx: Context<DeleteDomainFree>) -> Result<()> {
        let remaining = &ctx.remaining_accounts;
        require!(!remaining.is_empty(), ChiefburnerError::InvalidBatch);

        let mut total_rent: u64 = 0;

        for name_account in remaining.iter() {
            let rent = name_account.lamports();
            total_rent = total_rent.checked_add(rent).unwrap();

            let ix = Instruction {
                program_id: NAME_SERVICE_PROGRAM,
                accounts: vec![
                    AccountMeta::new(name_account.key(), false),
                    AccountMeta::new(ctx.accounts.owner.key(), true),
                    AccountMeta::new(ctx.accounts.cranker.key(), false),
                ],
                data: vec![3],
            };

            invoke(
                &ix,
                &[
                    name_account.to_account_info(),
                    ctx.accounts.owner.to_account_info(),
                    ctx.accounts.cranker.to_account_info(),
                ],
            )?;
        }

        emit!(DomainsDeleted {
            owner: ctx.accounts.owner.key(),
            cranker: ctx.accounts.cranker.key(),
            count: remaining.len() as u32,
            total_rent_reclaimed: total_rent,
            cranker_fee: total_rent,
        });

        Ok(())
    }
}

// ── Account structs ─────────────────────────────────────────────────

#[derive(Accounts)]
pub struct BurnAndClose<'info> {
    /// The owner of all token accounts. Must sign.
    #[account(mut)]
    pub owner: Signer<'info>,

    /// The cranker. Receives 5% of total reclaimed rent.
    /// CHECK: Any account can be the cranker.
    #[account(mut)]
    pub cranker: UncheckedAccount<'info>,

    /// The token program (all accounts in batch must use the same program).
    pub token_program: Interface<'info, TokenInterface>,
}

#[derive(Accounts)]
pub struct BurnAndCloseFree<'info> {
    /// The owner of all token accounts. Must sign.
    pub owner: Signer<'info>,

    /// The cranker. Receives 100% of total reclaimed rent.
    /// CHECK: Any account can be the cranker.
    #[account(mut)]
    pub cranker: UncheckedAccount<'info>,

    /// The token program (all accounts in batch must use the same program).
    pub token_program: Interface<'info, TokenInterface>,
}

#[derive(Accounts)]
pub struct BurnNft<'info> {
    /// The NFT owner. Must sign.
    #[account(mut)]
    pub owner: Signer<'info>,

    /// The cranker.
    /// CHECK: Any account can be the cranker.
    #[account(mut)]
    pub cranker: UncheckedAccount<'info>,

    /// SPL Token program (used by Metaplex CPI).
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

    /// The cranker. Receives 5% of total recovered rent.
    /// CHECK: Any account can be the cranker.
    #[account(mut)]
    pub cranker: UncheckedAccount<'info>,

    /// SPL Name Service program.
    /// CHECK: Must be the Name Service program.
    #[account(address = NAME_SERVICE_PROGRAM)]
    pub name_service_program: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct DeleteDomainFree<'info> {
    /// The domain owner. Must sign.
    pub owner: Signer<'info>,

    /// The cranker. Receives 100% of total recovered rent.
    /// CHECK: Any account can be the cranker.
    #[account(mut)]
    pub cranker: UncheckedAccount<'info>,

    /// SPL Name Service program.
    /// CHECK: Must be the Name Service program.
    #[account(address = NAME_SERVICE_PROGRAM)]
    pub name_service_program: UncheckedAccount<'info>,
}

// ── Events ──────────────────────────────────────────────────────────

#[event]
pub struct TokensBurned {
    pub owner: Pubkey,
    pub cranker: Pubkey,
    pub count: u32,
    pub total_rent_reclaimed: u64,
    pub cranker_fee: u64,
}

#[event]
pub struct NftsBurned {
    pub owner: Pubkey,
    pub cranker: Pubkey,
    pub count: u32,
    pub total_rent_reclaimed: u64,
    pub cranker_fee: u64,
}

#[event]
pub struct DomainsDeleted {
    pub owner: Pubkey,
    pub cranker: Pubkey,
    pub count: u32,
    pub total_rent_reclaimed: u64,
    pub cranker_fee: u64,
}

// ── Errors ──────────────────────────────────────────────────────────

#[error_code]
pub enum ChiefburnerError {
    #[msg("Invalid or mismatched account")]
    InvalidAccount,
    #[msg("Invalid batch: wrong number of remaining accounts")]
    InvalidBatch,
}
