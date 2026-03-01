use anchor_lang::prelude::*;
use anchor_lang::solana_program::{instruction::Instruction, program::invoke};
use anchor_spl::token_interface::{burn, close_account, Burn, CloseAccount, TokenInterface};

declare_id!("8CJi79SkfMYN29XX4WmBT8AmtvCrAzzrFYJsdai6oKwL");

/// Metaplex Token Metadata program
const METAPLEX_METADATA_PROGRAM: Pubkey =
    pubkey!("metaqbxxUerdq28cj1RbAWkYQm3ybzjb6a8bt518x1s");

/// SPL Name Service program
const NAME_SERVICE_PROGRAM: Pubkey =
    pubkey!("namesLPneVptA9Z5rqUDD9tMTWEJwofgaYwp8cawRkX");

/// Parse mint, owner, and amount from an SPL token account's raw data.
fn parse_token_account(data: &[u8]) -> Result<(Pubkey, Pubkey, u64)> {
    require!(data.len() >= 72, ChiefburnerError::InvalidAccount);
    let mint = Pubkey::try_from(&data[0..32]).unwrap();
    let owner = Pubkey::try_from(&data[32..64]).unwrap();
    let amount = u64::from_le_bytes(data[64..72].try_into().unwrap());
    Ok((mint, owner, amount))
}

#[program]
pub mod chiefburner {
    use super::*;

    /// Unified burn/close/delete instruction.
    ///
    /// Processes tokens, NFTs, and domains in a single call.
    /// All recovered rent goes to the owner, then cranker_fee_pct% is
    /// transferred from owner to cranker.
    ///
    /// remaining_accounts layout (in order):
    ///   num_tokens  pairs of (token_account, mint)
    ///   num_nfts    groups of 4 (metadata, mint, token_account, master_edition)
    ///   num_domains name_accounts
    pub fn burn_or_delete<'info>(
        ctx: Context<'_, '_, 'info, 'info, BurnOrDelete<'info>>,
        num_tokens: u16,
        num_nfts: u16,
        num_domains: u16,
        cranker_fee_pct: u8,
    ) -> Result<()> {
        require!(cranker_fee_pct <= 100, ChiefburnerError::InvalidFee);

        let expected =
            (num_tokens as usize) * 2 + (num_nfts as usize) * 4 + num_domains as usize;
        require!(expected > 0, ChiefburnerError::InvalidBatch);

        let remaining = &ctx.remaining_accounts;
        require!(remaining.len() == expected, ChiefburnerError::InvalidBatch);

        let owner_key = ctx.accounts.owner.key();
        let owner_info = ctx.accounts.owner.to_account_info();
        let lamports_before = owner_info.lamports();
        let mut offset: usize = 0;

        // ── Tokens: burn (if non-empty) + close ─────────────────────
        if num_tokens > 0 {
            let token_program = &ctx.accounts.token_program;
            let token_program_id = token_program.key();

            for _ in 0..num_tokens {
                let token_info = &remaining[offset];
                let mint_info = &remaining[offset + 1];
                offset += 2;

                require!(
                    *token_info.owner == token_program_id,
                    ChiefburnerError::InvalidAccount
                );

                let (mint_key, authority, amount) =
                    parse_token_account(&token_info.try_borrow_data()?)?;
                require!(authority == owner_key, ChiefburnerError::InvalidAccount);
                require!(
                    mint_key == mint_info.key(),
                    ChiefburnerError::InvalidAccount
                );

                if amount > 0 {
                    burn(
                        CpiContext::new(
                            token_program.to_account_info(),
                            Burn {
                                mint: mint_info.to_account_info(),
                                from: token_info.to_account_info(),
                                authority: owner_info.clone(),
                            },
                        ),
                        amount,
                    )?;
                }

                close_account(CpiContext::new(
                    token_program.to_account_info(),
                    CloseAccount {
                        account: token_info.to_account_info(),
                        destination: owner_info.clone(),
                        authority: owner_info.clone(),
                    },
                ))?;
            }
        }

        // ── NFTs: Metaplex BurnNft ──────────────────────────────────
        if num_nfts > 0 {
            require!(
                ctx.accounts.metadata_program.key() == METAPLEX_METADATA_PROGRAM,
                ChiefburnerError::InvalidProgram
            );

            for _ in 0..num_nfts {
                let metadata = &remaining[offset];
                let mint = &remaining[offset + 1];
                let token_account = &remaining[offset + 2];
                let master_edition = &remaining[offset + 3];
                offset += 4;

                invoke(
                    &Instruction {
                        program_id: METAPLEX_METADATA_PROGRAM,
                        accounts: vec![
                            AccountMeta::new(metadata.key(), false),
                            AccountMeta::new(owner_key, true),
                            AccountMeta::new(mint.key(), false),
                            AccountMeta::new(token_account.key(), false),
                            AccountMeta::new(master_edition.key(), false),
                            AccountMeta::new_readonly(
                                ctx.accounts.token_program.key(),
                                false,
                            ),
                        ],
                        data: vec![18], // BurnNft discriminator
                    },
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
        }

        // ── Domains: Name Service Delete ────────────────────────────
        if num_domains > 0 {
            require!(
                ctx.accounts.name_service_program.key() == NAME_SERVICE_PROGRAM,
                ChiefburnerError::InvalidProgram
            );

            for _ in 0..num_domains {
                let name_account = &remaining[offset];
                offset += 1;

                invoke(
                    &Instruction {
                        program_id: NAME_SERVICE_PROGRAM,
                        accounts: vec![
                            AccountMeta::new(name_account.key(), false),
                            AccountMeta::new(owner_key, true),
                            AccountMeta::new(owner_key, false), // refund to owner
                        ],
                        data: vec![3], // Delete discriminator
                    },
                    &[name_account.to_account_info(), owner_info.clone()],
                )?;
            }
        }

        // ── Fee settlement ──────────────────────────────────────────
        let total_rent = owner_info
            .lamports()
            .checked_sub(lamports_before)
            .unwrap_or(0);

        let cranker_fee = if cranker_fee_pct > 0
            && total_rent > 0
            && ctx.accounts.cranker.key() != owner_key
        {
            let fee = total_rent
                .checked_mul(cranker_fee_pct as u64)
                .unwrap()
                .checked_div(100)
                .unwrap();
            if fee > 0 {
                **owner_info.try_borrow_mut_lamports()? -= fee;
                **ctx
                    .accounts
                    .cranker
                    .to_account_info()
                    .try_borrow_mut_lamports()? += fee;
            }
            fee
        } else {
            0
        };

        emit!(BurnComplete {
            owner: owner_key,
            cranker: ctx.accounts.cranker.key(),
            num_tokens,
            num_nfts,
            num_domains,
            total_rent_reclaimed: total_rent,
            cranker_fee,
        });

        Ok(())
    }
}

// ── Accounts ────────────────────────────────────────────────────────

#[derive(Accounts)]
pub struct BurnOrDelete<'info> {
    /// Owner of all accounts being burned/closed. Must sign.
    #[account(mut)]
    pub owner: Signer<'info>,

    /// Cranker receiving the fee. Pass owner's address if no cranker.
    /// CHECK: Any account can be the cranker.
    #[account(mut)]
    pub cranker: UncheckedAccount<'info>,

    /// SPL Token or Token-2022 program (required for tokens and NFTs).
    pub token_program: Interface<'info, TokenInterface>,

    /// Metaplex Token Metadata program (validated when num_nfts > 0).
    /// CHECK: Validated in handler when needed.
    pub metadata_program: UncheckedAccount<'info>,

    /// SPL Name Service program (validated when num_domains > 0).
    /// CHECK: Validated in handler when needed.
    pub name_service_program: UncheckedAccount<'info>,
}

// ── Events ──────────────────────────────────────────────────────────

#[event]
pub struct BurnComplete {
    pub owner: Pubkey,
    pub cranker: Pubkey,
    pub num_tokens: u16,
    pub num_nfts: u16,
    pub num_domains: u16,
    pub total_rent_reclaimed: u64,
    pub cranker_fee: u64,
}

// ── Errors ──────────────────────────────────────────────────────────

#[error_code]
pub enum ChiefburnerError {
    #[msg("Invalid or mismatched account")]
    InvalidAccount,
    #[msg("Invalid batch: check remaining accounts count")]
    InvalidBatch,
    #[msg("Cranker fee must be 0-100")]
    InvalidFee,
    #[msg("Invalid program address")]
    InvalidProgram,
}
