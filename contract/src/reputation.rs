/// Arbitrator reputation: tracking, rating, and selection helpers.
///
/// Reputation is stored per-arbitrator and updated automatically when:
///   - A dispute is raised  → `total_disputes` incremented
///   - A dispute is resolved → `resolved_count`, `buyer_wins`, `seller_wins` updated
///   - A trade party rates  → `rating_sum`, `rating_count` updated
///
/// Selection helpers let callers pick the best available arbitrator from a
/// candidate list based on average rating and resolution rate.

use soroban_sdk::{Address, Env, Vec};

use crate::errors::ContractError;
use crate::storage::{
    get_arbitrator_reputation, has_arbitrator, has_rated, mark_rated,
    save_arbitrator_reputation,
};
use crate::types::ArbitratorReputation;
use crate::events;

// ---------------------------------------------------------------------------
// Rating
// ---------------------------------------------------------------------------

/// Submit a 1–5 star rating for the arbitrator of a resolved dispute.
///
/// Rules:
/// - `rater` must be the buyer or seller of the trade.
/// - Trade must be in `Disputed` status (i.e. dispute was raised; resolution
///   may still be pending — parties can rate once the dispute is active).
/// - Each party may rate at most once per trade.
pub fn rate_arbitrator(
    env: &Env,
    trade_id: u64,
    rater: &Address,
    arbitrator: &Address,
    buyer: &Address,
    seller: &Address,
    trade_status_is_disputed_or_completed: bool,
    stars: u32,
) -> Result<(), ContractError> {
    if stars < 1 || stars > 5 {
        return Err(ContractError::InvalidRating);
    }
    if rater != buyer && rater != seller {
        return Err(ContractError::Unauthorized);
    }
    if !trade_status_is_disputed_or_completed {
        return Err(ContractError::InvalidStatus);
    }
    if !has_arbitrator(env, arbitrator) {
        return Err(ContractError::ArbitratorNotRegistered);
    }
    if has_rated(env, trade_id, rater) {
        return Err(ContractError::AlreadyRated);
    }
    mark_rated(env, trade_id, rater);

    let mut rep = get_arbitrator_reputation(env, arbitrator);
    rep.rating_sum = rep.rating_sum.saturating_add(stars);
    rep.rating_count = rep.rating_count.saturating_add(1);
    save_arbitrator_reputation(env, arbitrator, &rep);

    events::emit_arb_rated(env, arbitrator.clone(), trade_id, rater.clone(), stars);
    events::emit_arb_rep_updated(
        env,
        arbitrator.clone(),
        rep.resolved_count,
        rep.rating_sum,
        rep.rating_count,
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Statistics helpers
// ---------------------------------------------------------------------------

/// Average star rating scaled by 100 (e.g. 450 = 4.50 stars).
/// Returns 0 if no ratings have been submitted.
pub fn average_rating_x100(rep: &ArbitratorReputation) -> u32 {
    if rep.rating_count == 0 {
        return 0;
    }
    rep.rating_sum
        .saturating_mul(100)
        .checked_div(rep.rating_count)
        .unwrap_or(0)
}

/// Resolution rate in basis points (0–10000).
/// Returns 0 if no disputes have been assigned.
pub fn resolution_rate_bps(rep: &ArbitratorReputation) -> u32 {
    if rep.total_disputes == 0 {
        return 0;
    }
    (rep.resolved_count as u64)
        .saturating_mul(10_000)
        .checked_div(rep.total_disputes as u64)
        .unwrap_or(0) as u32
}

/// Composite score used for selection: weighted sum of resolution rate and
/// average rating.  Both components are normalised to [0, 10000].
///
/// score = 0.6 × resolution_rate_bps + 0.4 × (avg_rating_x100 × 20)
///
/// The rating component is scaled so that 5 stars → 10000 bps:
///   avg_rating_x100 ∈ [100, 500]  →  × 20  →  [2000, 10000]
pub fn composite_score(rep: &ArbitratorReputation) -> u32 {
    let rr = resolution_rate_bps(rep) as u64;
    let ar = (average_rating_x100(rep) as u64).saturating_mul(20).min(10_000);
    // 60% resolution rate + 40% rating
    let score = rr.saturating_mul(6).saturating_add(ar.saturating_mul(4)) / 10;
    score.min(10_000) as u32
}

// ---------------------------------------------------------------------------
// Reputation-based selection
// ---------------------------------------------------------------------------

/// From `candidates`, return the registered arbitrator with the highest
/// composite score.  Ties are broken by the order in `candidates`.
///
/// Returns `Err(ArbitratorNotRegistered)` if no candidate is registered.
pub fn select_best_arbitrator(
    env: &Env,
    candidates: &Vec<Address>,
) -> Result<Address, ContractError> {
    let mut best: Option<Address> = None;
    let mut best_score: u32 = 0;

    for i in 0..candidates.len() {
        let candidate = candidates.get(i).unwrap();
        if !has_arbitrator(env, &candidate) {
            continue;
        }
        let rep = get_arbitrator_reputation(env, &candidate);
        let score = composite_score(&rep);
        if best.is_none() || score > best_score {
            best_score = score;
            best = Some(candidate);
        }
    }

    best.ok_or(ContractError::ArbitratorNotRegistered)
}

/// Return reputation records for all `arbitrators` in the same order.
pub fn get_reputations(
    env: &Env,
    arbitrators: &Vec<Address>,
) -> Vec<ArbitratorReputation> {
    let mut out = Vec::new(env);
    for i in 0..arbitrators.len() {
        let arb = arbitrators.get(i).unwrap();
        out.push_back(get_arbitrator_reputation(env, &arb));
    }
    out
}
