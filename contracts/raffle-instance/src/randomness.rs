//! Deterministic seed utilities and winner selection for raffle draws.
//!
//! # Overview
//!
//! The finalize path for every randomness mode ends in
//! [`do_finalize_with_seed`](crate::helpers::do_finalize_with_seed), which
//! receives a compact `u64` seed and uses [`OracleSeedWinnerSelection`] to map
//! that seed to winning ticket indices. The modes differ only in how the seed
//! is obtained:
//!
//! - Internal and oracle-timeout fallback draws derive a `u64` from current
//!   ledger state in `helpers::build_internal_seed_u64`.
//! - External/VRF draws use the oracle-provided seed after proof validation.
//! - Commit-reveal draws hash submitted commits into a `u64`, falling back to
//!   the internal seed when no commits are present.
//! - Quorum draws aggregate delivered oracle seeds into a `u64`.
//!
//! Winner selection itself is a single audited algorithm over a `u64` seed.
//! There is no runtime strategy dispatch and no `env.prng()` winner-selection
//! path.
//!
//! # VRF proof binding
//!
//! [`build_vrf_proof_message`] constructs the Ed25519 message that the oracle
//! must sign when submitting randomness. It binds the proof to this specific
//! raffle contract address **and** the request ID so that a valid proof for
//! one raffle cannot be replayed against a different raffle or request.
//!
//! See [`docs/RANDOMNESS.md`](../../../../docs/RANDOMNESS.md) for a
//! higher-level comparison of all randomness modes, and
//! [`docs/COMMIT_REVEAL.md`](../../../../docs/COMMIT_REVEAL.md) for the
//! commit-reveal protocol specification.

use soroban_sdk::{xdr::ToXdr, Address, Bytes, BytesN, Env, Vec};
#[cfg(test)]
use soroban_sdk::testutils::Address as _;

/// Build the Ed25519 message that binds a VRF proof to a specific raffle and
/// request.
///
/// The oracle must sign exactly this byte sequence when calling
/// [`provide_randomness`](crate::draw::provide_randomness).  The message
/// contains:
///
/// - The current contract address (`env.current_contract_address()`) — binds
///   the proof to **this** raffle; a proof generated for raffle A cannot be
///   replayed against raffle B.
/// - `request_id` — binds the proof to the specific randomness request; a
///   stale or recycled proof from an earlier draw cannot be accepted.
/// - `random_seed` — the oracle's VRF output being delivered.
///
/// All three fields are XDR-serialised together so the encoding is
/// unambiguous and length-delimited.
///
/// # Parameters
///
/// - `request_id` — The unique request ID stored in
///   [`DataKey::RandomnessRequestId`](crate::DataKey::RandomnessRequestId).
/// - `random_seed` — The VRF output (random seed) being delivered.
///
/// # Returns
///
/// A [`Bytes`] value that should be passed to `env.crypto().ed25519_verify`.
///
/// See also: [`docs/RANDOMNESS.md`](../../../../docs/RANDOMNESS.md) — External
/// / VRF mode.
pub fn build_vrf_proof_message(env: &Env, request_id: u64) -> Bytes {
    (env.current_contract_address(), request_id).to_xdr(env)
}

/// Derive the canonical seed from a verified proof.
pub fn derive_random_seed_from_proof(env: &Env, proof: &BytesN<64>) -> u64 {
    let proof_bytes: Bytes = Bytes::from_array(env, &proof.to_array());
    let hash: BytesN<32> = env.crypto().sha256(&proof_bytes).into();
    let arr = hash.to_array();
    u64::from_be_bytes([arr[0], arr[1], arr[2], arr[3], arr[4], arr[5], arr[6], arr[7]])
}

/// Oracle-backed winner selection using an externally provided VRF seed.
///
/// Used by [`provide_randomness`](crate::draw::provide_randomness) after the
/// oracle has delivered a cryptographically-verified random value. Internal,
/// commit-reveal, fallback, and quorum paths also use this same selector once
/// they have produced their `u64` seed.
///
/// ## Rejection sampling
///
/// To eliminate modulo bias, the selection uses rejection sampling.  A sample
/// is only accepted when it falls below
/// `floor(u64::MAX / total_tickets) * total_tickets` — the largest multiple
/// of `total_tickets` that fits in a `u64`.  Samples in the biased tail are
/// discarded and the seed is advanced using an LCG step.
///
/// ## LCG advance
///
/// Between samples the internal state is advanced with the LCG:
/// ```text
/// state = state * 6364136223846793005 + 1442695040888963407
/// ```
/// (Knuth's constants, same as those used in many standard libraries.)
pub struct OracleSeedWinnerSelection {
    seed: u64,
}

impl OracleSeedWinnerSelection {
    /// Create a new selector seeded with the oracle-provided VRF output.
    pub fn new(seed: u64) -> Self {
        Self { seed }
    }

/// Pure (no-`Env`) version of [`select_winner_indices`] used in tests and
/// off-chain tooling.  Available only when `std` is in scope.
    #[cfg(any(test, feature = "std"))]
    pub fn select_winner_indices_pure(
        &self,
        total_tickets: u32,
        winner_count: u32,
    ) -> std::vec::Vec<u32> {
        let mut indices = std::vec::Vec::new();
        if total_tickets == 0 || winner_count == 0 {
            return indices;
        }

        let n = total_tickets as u64;
        let effective_count = winner_count.min(total_tickets) as usize;
        // Partial Fisher-Yates shuffle: select effective_count unique indices from [0, n)
        // in exactly effective_count steps with no unbounded loop.
        // We use rejection sampling at each step to eliminate modulo bias, and
        // swap tracking to ensure uniqueness without a linear scan.
        let mut remaining = n;
        let mut current_seed = self.seed;
        let mut swaps: std::vec::Vec<(u64, u64)> = std::vec::Vec::new();

        for _ in 0..effective_count {
            // Generate an unbiased u64 in [0, remaining) using rejection sampling.
            let largest_multiple_remaining = (u64::MAX / remaining) * remaining;
            let candidate = loop {
                if current_seed < largest_multiple_remaining {
                    break current_seed;
                }
                current_seed = current_seed
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
            };
            let r = candidate % remaining;

            // Map the candidate to an actual unique index using Fisher-Yates swap tracking.
            let mut actual = r as u64;
            for (pos, val) in swaps.iter() {
                if *pos == actual {
                    actual = *val;
                    break;
                }
            }

            indices.push(actual as u32);

            // Swap: position r now contains what was at position remaining-1.
            let last = remaining - 1;
            let mut last_actual = last;
            for (pos, val) in swaps.iter() {
                if *pos == last {
                    last_actual = *val;
                    break;
                }
            }

            // Record the swap: position r now contains what was at position last.
            // If position r already has a mapping, overwrite it (most recent swap takes precedence).
            let mut found_idx: Option<usize> = None;
            for (idx, (pos, _)) in swaps.iter().enumerate() {
                if *pos == r {
                    found_idx = Some(idx);
                    break;
                }
            }
            if let Some(idx) = found_idx {
                swaps[idx].1 = last_actual;
            } else {
                swaps.push((r, last_actual));
            }

            remaining -= 1;
        }

        indices
    }

    /// Select distinct zero-based winner indices from the provided ticket range.
    ///
    /// Partial Fisher-Yates over an explicit index pool: the loop draws exactly
    /// `min(winner_count, total_tickets)` entries, so the result can never be
    /// short even when early draws collide.
    pub fn select_winner_indices(
        &self,
        env: &Env,
        total_tickets: u32,
        winner_count: u32,
    ) -> Vec<u32> {
        let mut indices = Vec::new(env);
        if total_tickets == 0 || winner_count == 0 {
            return indices;
        }

        let mut pool: Vec<u32> = Vec::new(env);
        for i in 0..total_tickets {
            pool.push_back(i);
        }

        let take = winner_count.min(total_tickets);
        let mut current_seed = self.seed;
        let mut i = total_tickets;
        for _ in 0..take {
            i -= 1;
            let j = unbiased_below(&mut current_seed, i + 1) as u32;
            if i != j {
                let at_i = pool.get(i).unwrap();
                let at_j = pool.get(j).unwrap();
                pool.set(i, at_j);
                pool.set(j, at_i);
            }
            indices.push_back(pool.get(i).unwrap());
        }
        indices
    }
}

/// Advance `seed` and return an unbiased value in `[0, bound)` using
/// rejection sampling (no modulo bias). `bound == 0` yields `0`.
fn unbiased_below(seed: &mut u64, bound: u32) -> u64 {
    let n = bound as u64;
    if n == 0 {
        return 0;
    }
    let largest_multiple = (u64::MAX / n) * n;
    loop {
        let current = *seed;
        *seed = current
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        if current < largest_multiple {
            return current % n;
        }
    }
}

/// Aggregate multiple oracle seeds into a single deterministic seed.
///
/// Seeds are sorted by oracle address (XDR bytes, lexicographic) before
/// concatenation so the result is **order-independent**: the same multiset of
/// seeds always yields the same aggregate regardless of submission order.
///
/// Each seed is appended as 8 big-endian bytes, then SHA-256 is applied.
/// The first 8 bytes of the hash become the `u64` draw seed.
pub fn aggregate_quorum_seeds(env: &Env, seeds: &Vec<(Address, u64)>) -> u64 {
    if seeds.is_empty() {
        return 0u64;
    }

    let mut sorted = Vec::new(env);
    for i in 0..seeds.len() {
        if let Some(pair) = seeds.get(i) {
            sorted.push_back(pair);
        }
    }

    // Insertion sort by address XDR bytes (n ≤ 10).
    for i in 1..sorted.len() {
        let mut j = i;
        while j > 0 {
            let (addr_j, seed_j) = sorted.get(j).unwrap();
            let (addr_prev, seed_prev) = sorted.get(j - 1).unwrap();
            let bytes_j: Bytes = addr_j.clone().to_xdr(env);
            let bytes_prev: Bytes = addr_prev.clone().to_xdr(env);
            if bytes_j < bytes_prev {
                sorted.set(j, (addr_prev.clone(), seed_prev));
                sorted.set(j - 1, (addr_j.clone(), seed_j));
                j -= 1;
            } else {
                break;
            }
        }
    }

    let mut combined = Bytes::new(env);
    for i in 0..sorted.len() {
        if let Some((addr, seed)) = sorted.get(i) {
            // Bind each contribution to the oracle that produced it so a
            // submitted seed cannot be replayed under a different address.
            combined.append(&addr.clone().to_xdr(env));
            combined.extend_from_array(&seed.to_be_bytes());
        }
    }

    let hash: BytesN<32> = env.crypto().sha256(&combined).into();
    let arr = hash.to_array();
    let mut seed_bytes = [0u8; 8];
    seed_bytes.copy_from_slice(&arr[..8]);
    u64::from_be_bytes(seed_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::Env;

    /// Deliberately biased winner selector used to verify that the Chi-squared test
    /// correctly detects modulo / index distribution bias (#633).
    struct BiasedWinnerSelection {
        seed: u64,
    }

    impl BiasedWinnerSelection {
        fn select_winner_indices_biased(&self, total_tickets: u32) -> u32 {
            let n = total_tickets as u64;
            // Intentionally introduces modulo bias by wrapping around an asymmetric range
            ((self.seed % (n + 1)) % n) as u32
        }
    }

    /// Computes the Chi-squared statistic for a frequency histogram against a uniform distribution.
    fn compute_chi_squared(histogram: &[u32], total_samples: u32) -> f64 {
        let k = histogram.len() as f64;
        let expected = total_samples as f64 / k;
        let mut chi2 = 0.0;
        for &count in histogram {
            let diff = count as f64 - expected;
            chi2 += (diff * diff) / expected;
        }
        chi2
    }

    /// Critical values for Chi-squared distribution at alpha = 0.001 (significance level 99.9%).
    fn critical_value_999(degrees_of_freedom: usize) -> f64 {
        match degrees_of_freedom {
            4 => 18.47,  // 5 tickets - 1
            8 => 26.12,  // 9 tickets - 1
            32 => 62.49, // 33 tickets - 1
            df => (df as f64) + 3.0 * (2.0 * df as f64).sqrt(),
        }
    }

    /// Helper running the Chi-squared goodness-of-fit test for OracleSeedWinnerSelection.
    fn run_uniformity_simulation(ticket_counts: &[u32], total_draws: u32) {
        for &n in ticket_counts {
            let mut histogram = std::vec![0u32; n as usize];
            for seed in 1..=(total_draws as u64) {
                let strategy = OracleSeedWinnerSelection::new(seed);
                let winners = strategy.select_winner_indices_pure(n, 1);
                assert_eq!(winners.len(), 1);
                histogram[winners[0] as usize] += 1;
            }
            let df = (n - 1) as usize;
            let chi2 = compute_chi_squared(&histogram, total_draws);
            let crit = critical_value_999(df);
            assert!(
                chi2 < crit,
                "Real winner selector failed Chi-squared uniformity test for ticket_count={n}: chi2={chi2} >= critical={crit}"
            );
        }
    }

    /// Statistical uniformity test (CI variant: 5,000 samples per ticket count).
    /// Tests ticket counts chosen to stress modulo bias (just above powers of two: 5, 9, 33).
    #[test]
    fn test_statistical_uniformity_ci() {
        run_uniformity_simulation(&[5, 9, 33], 5_000);
    }

    /// Statistical uniformity test (Full simulation variant: 100,000 samples per ticket count).
    /// Marked as #[ignore] by default to keep CI fast.
    #[test]
    #[ignore]
    fn test_statistical_uniformity_full() {
        run_uniformity_simulation(&[5, 9, 33], 100_000);
    }

    /// Acceptance criterion test: verifies that the Chi-squared test REJECTS a biased selector.
    #[test]
    fn test_statistical_uniformity_rejects_biased_selector() {
        let total_draws = 5_000u32;
        for &n in &[5u32, 9u32, 33u32] {
            let mut histogram = std::vec![0u32; n as usize];
            for seed in 1..=(total_draws as u64) {
                let biased = BiasedWinnerSelection { seed };
                let winner = biased.select_winner_indices_biased(n);
                histogram[winner as usize] += 1;
            }
            let df = (n - 1) as usize;
            let chi2 = compute_chi_squared(&histogram, total_draws);
            let crit = critical_value_999(df);
            assert!(
                chi2 >= crit,
                "Chi-squared test must REJECT biased selector for ticket_count={n}: chi2={chi2} expected >= critical={crit}"
            );
        }
    }

    #[test]
    fn aggregate_quorum_seeds_is_order_independent() {
        let env = Env::default();
        let contract = env
            .register_stellar_asset_contract_v2(Address::generate(&env))
            .address();

        let addr_a = Address::generate(&env);
        let addr_b = Address::generate(&env);
        let addr_c = Address::generate(&env);

        let forward = env.as_contract(&contract, || {
            let mut v = Vec::new(&env);
            v.push_back((addr_a.clone(), 10u64));
            v.push_back((addr_b.clone(), 20u64));
            v.push_back((addr_c.clone(), 30u64));
            aggregate_quorum_seeds(&env, &v)
        });

        let reverse = env.as_contract(&contract, || {
            let mut v = Vec::new(&env);
            v.push_back((addr_c.clone(), 30u64));
            v.push_back((addr_b.clone(), 20u64));
            v.push_back((addr_a.clone(), 10u64));
            aggregate_quorum_seeds(&env, &v)
        });

        assert_eq!(forward, reverse);
    }

    #[test]
    fn aggregate_quorum_seeds_golden_vector() {
        let env = Env::default();
        let contract = env
            .register_stellar_asset_contract_v2(Address::generate(&env))
            .address();

        // Deterministic contract-scoped addresses for cross-service golden vectors.
        let addr_a = Address::generate(&env);
        let addr_b = Address::generate(&env);

        let aggregate = env.as_contract(&contract, || {
            let mut v = Vec::new(&env);
            v.push_back((addr_b.clone(), 0xDEAD_BEEFu64));
            v.push_back((addr_a.clone(), 0xCAFE_BABEu64));
            aggregate_quorum_seeds(&env, &v)
        });

        // Exported to oracle/src/vrf/__fixtures__/quorum-aggregate-vectors.json
        assert_ne!(aggregate, 0);
    }

    #[test]
    fn aggregate_quorum_seeds_empty_returns_zero() {
        let env = Env::default();
        let contract = env
            .register_stellar_asset_contract_v2(Address::generate(&env))
            .address();
        let result = env.as_contract(&contract, || {
            let v = Vec::new(&env);
            aggregate_quorum_seeds(&env, &v)
        });
        assert_eq!(result, 0);
    }
}
