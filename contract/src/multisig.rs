/// Multi-signature arbitration for high-value trades.
///
/// Flow:
///   1. Admin creates a trade with `create_multisig_trade`, supplying a
///      `MultiSigConfig` (arbitrators list + threshold + voting_timeout_seconds).
///   2. When a dispute is raised the voting window opens automatically.
///   3. Each arbitrator calls `cast_vote` with their preferred resolution.
///   4. Once `threshold` arbitrators agree on the same resolution, consensus
///      is reached and `resolve_dispute` can be called to execute it.
///   5. If the voting window expires without consensus, the admin may call
///      `resolve_expired_dispute` to handle the disagreement (refund buyer).

use soroban_sdk::{Address, Env, Vec};

use crate::errors::ContractError;
use crate::events;
use crate::storage::{
    get_all_votes_for_trade, get_arbitrator_vote, get_trade, has_arbitrator,
    has_arbitrator_voted, save_arbitrator_vote, save_trade,
};
use crate::types::{
    ArbitrationConfig, ArbitratorVote, DisputeResolution, MultiSigConfig, Trade,
    TradeStatus, VotingSummary,
};

// ---------------------------------------------------------------------------
// Voting
// ---------------------------------------------------------------------------

/// Cast a vote on a disputed multi-sig trade.
///
/// - `arbitrator` must be in the trade's arbitrator list and registered.
/// - Trade must be in `Disputed` status.
/// - Each arbitrator may vote at most once.
pub fn cast_vote(
    env: &Env,
    trade_id: u64,
    arbitrator: &Address,
    resolution: DisputeResolution,
) -> Result<(), ContractError> {
    let trade = get_trade(env, trade_id)?;
    if trade.status != TradeStatus::Disputed {
        return Err(ContractError::InvalidStatus);
    }

    let config = multisig_config(&trade)?;

    // Verify arbitrator is in the panel
    if !is_panel_member(arbitrator, &config.arbitrators) {
        return Err(ContractError::Unauthorized);
    }
    if !has_arbitrator(env, arbitrator) {
        return Err(ContractError::ArbitratorNotRegistered);
    }
    if has_arbitrator_voted(env, trade_id, arbitrator) {
        return Err(ContractError::AlreadyVoted);
    }

    // Check voting window has not expired
    if is_voting_expired(env, &config) {
        return Err(ContractError::VotingExpired);
    }

    arbitrator.require_auth();

    let vote = ArbitratorVote {
        arbitrator: arbitrator.clone(),
        resolution,
        timestamp: env.ledger().timestamp(),
    };
    save_arbitrator_vote(env, trade_id, arbitrator, &vote);

    events::emit_arbitrator_vote_cast(env, trade_id, arbitrator.clone(), resolution.clone());
    Ok(())
}

// ---------------------------------------------------------------------------
// Consensus check
// ---------------------------------------------------------------------------

/// Compute the current voting state for a multi-sig trade.
pub fn voting_summary(env: &Env, trade_id: u64) -> Result<VotingSummary, ContractError> {
    let trade = get_trade(env, trade_id)?;
    let config = multisig_config(&trade)?;

    let votes = get_all_votes_for_trade(env, trade_id, &config.arbitrators);
    let votes_cast = votes.len() as u32;
    let total_arbitrators = config.arbitrators.len() as u32;
    let expired = is_voting_expired(env, &config);

    // Count votes per resolution variant
    let mut release_to_buyer: u32 = 0;
    let mut release_to_seller: u32 = 0;
    let mut partial_votes: Vec<(u32, u32)> = Vec::new(env); // (buyer_bps, count)

    for i in 0..votes.len() {
        let vote = votes.get(i).unwrap();
        match vote.resolution {
            DisputeResolution::ReleaseToBuyer => release_to_buyer += 1,
            DisputeResolution::ReleaseToSeller => release_to_seller += 1,
            DisputeResolution::Partial { buyer_bps } => {
                // Group by buyer_bps value
                let mut found = false;
                for j in 0..partial_votes.len() {
                    let (bps, count) = partial_votes.get(j).unwrap();
                    if bps == buyer_bps {
                        partial_votes.set(j, (bps, count + 1));
                        found = true;
                        break;
                    }
                }
                if !found {
                    partial_votes.push_back((buyer_bps, 1));
                }
            }
        }
    }

    // Find consensus resolution (first to reach threshold)
    let threshold = config.threshold;
    let mut consensus: Option<DisputeResolution> = None;

    if release_to_buyer >= threshold {
        consensus = Some(DisputeResolution::ReleaseToBuyer);
    } else if release_to_seller >= threshold {
        consensus = Some(DisputeResolution::ReleaseToSeller);
    } else {
        for i in 0..partial_votes.len() {
            let (bps, count) = partial_votes.get(i).unwrap();
            if count >= threshold {
                consensus = Some(DisputeResolution::Partial { buyer_bps: bps });
                break;
            }
        }
    }

    Ok(VotingSummary {
        total_arbitrators,
        votes_cast,
        threshold,
        consensus_resolution: consensus.clone(),
        has_consensus: consensus.is_some(),
        voting_expired: expired,
    })
}

// ---------------------------------------------------------------------------
// Disagreement / expiry handling
// ---------------------------------------------------------------------------

/// Called when the voting window has expired without consensus.
/// Refunds the buyer (safe default) and marks the trade as Completed.
/// Only the admin should call this.
pub fn resolve_expired_dispute(
    env: &Env,
    trade_id: u64,
    admin: &Address,
) -> Result<DisputeResolution, ContractError> {
    let trade = get_trade(env, trade_id)?;
    if trade.status != TradeStatus::Disputed {
        return Err(ContractError::InvalidStatus);
    }
    let config = multisig_config(&trade)?;
    if !is_voting_expired(env, &config) {
        return Err(ContractError::VotingNotExpired);
    }
    admin.require_auth();
    // Default: refund buyer on disagreement
    Ok(DisputeResolution::ReleaseToBuyer)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn multisig_config(trade: &Trade) -> Result<MultiSigConfig, ContractError> {
    match &trade.arbitrator {
        Some(ArbitrationConfig::MultiSig(cfg)) => Ok(cfg.clone()),
        _ => Err(ContractError::InvalidStatus),
    }
}

fn is_panel_member(arbitrator: &Address, panel: &Vec<Address>) -> bool {
    for i in 0..panel.len() {
        if panel.get(i).unwrap() == *arbitrator {
            return true;
        }
    }
    false
}

fn is_voting_expired(env: &Env, config: &MultiSigConfig) -> bool {
    if let Some(started_at) = config.voting_started_at {
        let now = env.ledger().timestamp();
        now > started_at.saturating_add(config.voting_timeout_seconds)
    } else {
        false
    }
}
