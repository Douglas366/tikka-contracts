#![no_std]
#![cfg_attr(not(test), deny(clippy::unwrap_used))]
#![warn(clippy::arithmetic_side_effects)]
#![deny(unused)]

#[cfg(test)]
extern crate std;

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, Address, BytesN, Env, String, Vec,
};

#[cfg(test)]
use self::randomness::build_vrf_proof_message;
#[cfg(test)]
use soroban_sdk::{token, Bytes};
#[cfg(test)]
use soroban_sdk::testutils::Address as _;
#[cfg(test)]
use soroban_sdk::vec;

mod admin;
mod attestation;
mod claim;
mod draw;
mod events;
mod helpers;
mod init;
mod randomness;
mod tickets;
mod views;

pub(crate) use helpers::{
    calculate_tier_prize, read_raffle, require_admin, require_global_not_paused,
    require_not_paused, request_randomness, transition_status, transition_to_drawing,
    validate_token_address, write_raffle, Guard,
};
#[cfg(any(test, feature = "testutils"))]
pub use helpers::assert_solvent;

#[cfg(any(test, feature = "testutils"))]
fn assert_solvent_after_success<T>(env: &Env, result: &Result<T, Error>) {
    if result.is_ok() {
        helpers::assert_solvent(env);
    }
}

use raffle_shared::{
    constants::{
        DEFAULT_CLAIM_EXPIRY_SECONDS, DEFAULT_CLAIM_LOCKUP_SECONDS, DEFAULT_SWAP_DEADLINE_SECONDS,
        EMERGENCY_WITHDRAW_DELAY_SECONDS, MAX_CLAIM_LOCKUP_SECONDS, MAX_DESCRIPTION_LENGTH,
        MAX_PRIZES, MAX_PRIZE_AMOUNT, MAX_PROTOCOL_FEE_BP, MAX_SWAP_DEADLINE_SECONDS,
        MAX_TICKETS_LIMIT, MIN_CLAIM_EXPIRY_SECONDS, MIN_TICKET_PRICE, ORACLE_TIMEOUT_LEDGERS,
    },
    exceeds_internal_randomness_cap, BuyQuote, CancelReason, FairnessData, QuorumConfig,
    RaffleConfig, RaffleStats, RaffleStatus, RandomnessSource, RandomnessType, Ticket,
};

use crate::events::RaffleCreated;

const RANDOMNESS_MIN_DELAY_LEDGERS: u32 = 10;

#[contract]
pub struct RaffleInstance;

/// Test-only alias so unit tests may register the contract as `Contract`.
#[cfg(test)]
pub use RaffleInstance as Contract;

/// Test-only alias for [`RaffleInstanceClient`].
#[cfg(test)]
pub type ContractClient<'a> = RaffleInstanceClient<'a>;

/// Test-only stand-in for the factory contract. The instance invokes
/// `record_volume` / `track_participant` on its factory during ticket
/// purchases, so tests register this stub to keep those calls on-ledger.
#[cfg(test)]
#[contract]
pub struct MockFactory;

#[cfg(test)]
#[contractimpl]
impl MockFactory {
    pub fn is_global_paused(_env: Env) -> bool {
        false
    }

    pub fn record_volume(_env: Env, _payment_token: Address, _total_price: i128) {}

    pub fn track_participant(_env: Env, _buyer: Address) {}

    pub fn record_leaderboard_entry(
        _env: Env,
        _raffle_id: Address,
        _tickets: i128,
        _prize_amount: i128,
        _volume: i128,
    ) {
    }
}

#[contracttype]
#[derive(Clone)]
pub struct Raffle {
    pub creator: Address,
    pub description: String,
    /// Unix timestamp after which ticket sales close and `finalize_raffle`
    /// may transition the raffle out of `Active`. The boundary is exclusive:
    /// sales are open while `ledger_timestamp < end_time`, and the deadline
    /// is reached starting at `ledger_timestamp == end_time` (enforced
    /// identically by `buy_tickets`, `buy_tickets_for`, and
    /// `finalize_raffle`; see `docs/GLOSSARY.md` § "End Time"). Ignored when
    /// `no_deadline` is `true`.
    pub end_time: u64,
    /// If true, `end_time` is not enforced and the raffle can remain
    /// `Active` indefinitely until `max_tickets` sells out or the raffle is
    /// cancelled.
    pub no_deadline: bool,
    pub max_tickets: u32,
    pub max_tickets_per_tx: u32,
    pub max_tickets_per_address: u32,
    pub min_tickets: u32,
    pub allow_multiple: bool,
    pub ticket_price: i128,
    pub payment_token: Address,
    /// The token used for prize deposit and claims. The current initializer
    /// always sets this to `payment_token`; the config override is not wired.
    pub prize_token: Address,
    pub prize_amount: i128,
    pub prizes: Vec<u32>,
    pub tickets_sold: u32,
    pub status: RaffleStatus,
    pub prize_deposited: bool,
    /// Winners, indexed by prize tier. Claim state lives on each winner.
    pub winners: Vec<Winner>,
    pub randomness_source: RandomnessSource,
    pub oracle_address: Option<Address>,
    pub protocol_fee_bp: u32,
    pub treasury_address: Option<Address>,
    pub swap_router: Option<Address>,
    pub tikka_token: Option<Address>,
    pub finalized_at: Option<u64>,
    pub claim_lockup_seconds: u64,
    pub claim_expiry_seconds: u64,
    pub swap_deadline_seconds: u64,
    pub ticket_sales_paused: bool,
    /// The percentage of max_tickets covered by the early bird discount (0 to disable).
    pub early_bird_ticket_percentage: u32,
    /// The discount amount specified in basis points.
    pub early_bird_discount_bp: u32,
    pub metadata_hash: BytesN<32>,
    pub unique_winners: bool,
    /// Tiered bundle pricing from config (validated at init).
    pub bundles: soroban_sdk::Vec<raffle_shared::TicketBundle>,
    pub nft_contract: Option<Address>,
}

#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Winner {
    pub address: Address,
    pub claimed: bool,
}

#[contracttype]
#[derive(Clone)]
pub struct FairnessMetadata {
    pub seed: u64,
    pub randomness_source: RandomnessSource,
    pub winning_ticket_indices: Vec<u32>,
    pub draw_timestamp: u64,
    pub draw_sequence: u32,
    pub unique_winners: bool,
    pub quorum_contributions: Option<Vec<(Address, u64)>>,
}

#[soroban_sdk::contracttype]
#[derive(Clone)]
pub enum DataKey {
    Raffle,
    TicketCount(Address),
    Ticket(u32),
    TicketRefunded(u32),
    Factory,
    ReentrancyGuard,
    Paused,
    Admin,
    RandomnessSeed,
    RandomnessRequested,
    RandomnessRequestLedger,
    RandomnessRequestId,
    FinishTime,
    AccumulatedFees,
    CommitEntry(u32),
    DrawingLock,
    TicketBuyers,
    OwnerTickets(Address),
    PendingAdminCancel,
    QuorumSeed(Address),
    QuorumCommit(Address),
    QuorumSubmittedOracles,
    MetadataHash,
    LastBumpedIndex,
}

#[contracttype]
#[derive(Clone)]
pub struct CommitRevealEntry {
    pub committer: Address,
    pub hash: BytesN<32>,
    /// Pre-image revealed by the committer, once opened during finalization.
    /// `None` until [`reveal_commit`](crate::draw::reveal_commit) succeeds.
    pub revealed: Option<BytesN<32>>,
}

// `export = false`: the Soroban spec XDR caps a `ScSpecUdtErrorEnumV0` at 50
// cases (`VecM<_, 50>`), and this enum already carries 50+ codes (see
// `docs/ERRORS.md`, which is the CI-enforced source of truth for error codes
// and frontend messages). Exporting the spec would make the derive panic with
// `LengthExceedsMax`, so the spec entry is omitted while every code and name
// is preserved.
#[contracterror(export = false)]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
pub enum Error {
    /// Raffle is not in an active state for ticket sales. Code 2.
    RaffleInactive = 2,
    /// All tickets have been sold. Code 3.
    TicketsSoldOut = 3,
    /// Caller balance is insufficient for the operation. Code 4.
    InsufficientFunds = 4,
    /// Caller is not authorized for this action. Code 5.
    NotAuthorized = 5,
    /// External randomness requested but oracle is not configured. Code 6.
    OracleNotSet = 6,
    /// Randomness was already requested for this draw. Code 7.
    RandomnessAlreadyRequested = 7,
    /// No pending randomness request exists. Code 8.
    NoRandomnessRequest = 8,
    /// Fallback randomness cannot be used yet. Code 9.
    FallbackTooEarly = 9,
    /// Prize has not been deposited by the creator. Code 11.
    PrizeNotDeposited = 11,
    /// Prize tier was already claimed or swept. Code 12.
    PrizeAlreadyClaimed = 12,
    /// Prize deposit was already completed. Code 13.
    PrizeAlreadyDeposited = 13,
    /// Caller is not the winner for this tier. Code 14.
    NotWinner = 14,
    /// Claim or sweep attempted before the configured delay elapsed. Code 15.
    ClaimTooEarly = 15,
    /// One or more input parameters are invalid. Code 21.
    InvalidParameters = 21,
    /// Ticket quantity is out of range. Code 22.
    InvalidQuantity = 22,
    /// Raffle status does not allow this operation. Code 23.
    InvalidStatus = 23,
    /// Contract is paused. Code 24.
    ContractPaused = 24,
    /// Requested lifecycle transition is not allowed. Code 25.
    InvalidStateTransition = 25,
    /// Raffle end time has passed. Code 26.
    RaffleExpired = 26,
    /// Address already holds a ticket when multiples are disallowed. Code 32.
    MultipleTicketsNotAllowed = 32,
    /// No tickets were sold. Code 33.
    NoTicketsSold = 33,
    /// Ticket record was not found. Code 34.
    TicketNotFound = 34,
    /// Integer overflow in a contract calculation. Code 41.
    ArithmeticOverflow = 41,
    /// Contract initialization was already performed. Code 42.
    AlreadyInitialized = 42,
    /// Contract has not been initialized. Code 43.
    NotInitialized = 43,
    /// Reentrant call detected. Code 44.
    Reentrancy = 44,
    /// Token transfer failed. Code 45.
    TokenTransferFailed = 45,
    /// No active tickets remain for the operation. Code 46.
    NoActiveTickets = 46,
    /// Token swap deadline has passed. Code 47.
    DeadlinePassed = 47,
    /// Swap output below slippage tolerance. Code 48.
    SlippageExceeded = 48,
    /// Index is out of bounds. Code 49.
    InvalidIndex = 49,
    /// More prize tiers configured than tickets sold. Code 50.
    MorePrizesThanTickets = 50,
    /// Computed prize amount is zero. Code 51.
    ZeroPrize = 51,
    /// Token address is invalid or unsupported. Code 52.
    InvalidTokenAddress = 52,
    /// Prize tier count exceeds protocol maximum. Code 53.
    TooManyPrizes = 53,
    /// Emergency withdraw attempted before the delay elapsed. Code 54.
    EmergencyTooEarly = 54,
    /// Minimum tickets exceed maximum tickets. Code 55.
    InvalidTicketRange = 55,
    /// Accumulated fees are below the requested withdrawal. Code 56.
    InsufficientAccumulatedFees = 56,
    /// Ticket purchase exceeds per-transaction cap. Code 58.
    ExceedsMaxTicketsPerTx = 58,
    DrawingAlreadyInProgress = 59,
    /// Draw has already completed. Code 61.
    DrawingAlreadyComplete = 61,
    /// End time is in the past or otherwise invalid. Code 62.
    InvalidEndTime = 62,
    /// Admin address is zero, self, or otherwise invalid. Code 63.
    InvalidAdminAddress = 63,
    /// Randomness callback received before the minimum delay. Code 64.
    RandomnessTooEarly = 64,
    CancelTimelockActive = 65,
    CancelNotScheduled = 66,
    ExceedsMaxTicketsPerAddress = 67,
    OracleNotRegistered = 68,
    DuplicateOracleSubmission = 69,
    CommitAlreadySubmitted = 70,
    /// Randomness callback arrived before the raffle entered the drawing phase.
    /// Code 71.
    DrawingNotStarted = 71,
    /// Reveal was submitted for an oracle that has not committed yet. Code 72.
    MissingCommit = 72,
    /// Revealed pre-image does not hash to the commitment stored for this
    /// oracle. Code 73.
    CommitMismatch = 73,
    /// Reveal attempted before `k` commitments were recorded. Code 74.
    TooFewCommits = 74,
    /// Configured randomness source is too weak for the configured prize
    /// amount. Code 75.
    RandomnessSourceTooWeakForPrize = 75,
}

#[contractimpl]
impl RaffleInstance {
    pub fn init(
        env: Env,
        factory: Address,
        admin: Address,
        creator: Address,
        config: RaffleConfig,
    ) -> Result<(), Error> {
        if config.description.len() > MAX_DESCRIPTION_LENGTH {
            return Err(Error::InvalidParameters);
        }

        let now = env.ledger().timestamp();
        if config.no_deadline && config.end_time != 0 {
            return Err(Error::InvalidParameters);
        }
        if !config.no_deadline && config.end_time <= now {
            return Err(Error::InvalidParameters);
        }
        // Explicit check: end_time must be either 0 (no deadline) or in the future
        if config.end_time != 0 && config.end_time <= now {
            return Err(Error::InvalidEndTime);
        }
        if config.max_tickets == 0 || config.max_tickets > MAX_TICKETS_LIMIT {
            return Err(Error::InvalidParameters);
        }
        if config.max_tickets < config.min_tickets {
            return Err(Error::InvalidTicketRange);
        }
        if config.max_tickets_per_tx == 0 || config.max_tickets_per_tx > config.max_tickets {
            return Err(Error::InvalidParameters);
        }
        if config.max_tickets_per_address > 0
            && config.max_tickets_per_address > config.max_tickets
        {
            return Err(Error::InvalidParameters);
        }

        if config.ticket_price < MIN_TICKET_PRICE {
            return Err(Error::InvalidParameters);
        }
        if config.prize_amount < config.ticket_price {
            return Err(Error::InvalidParameters);
        }
        if config.prize_amount > MAX_PRIZE_AMOUNT {
            return Err(Error::InvalidParameters);
        }
        if exceeds_internal_randomness_cap(&config.randomness_source, config.prize_amount) {
            return Err(Error::RandomnessSourceTooWeakForPrize);
        }
        if config.prizes.is_empty() {
            return Err(Error::InvalidParameters);
        }
        if config.prizes.len() > MAX_PRIZES {
            return Err(Error::TooManyPrizes);
        }
        if config.prizes.len() > config.max_tickets {
            return Err(Error::InvalidParameters);
        }
        let mut total_prizes_bp = 0u32;
        for prize_bp in config.prizes.iter() {
            if prize_bp > 10_000 {
                return Err(Error::InvalidParameters);
            }
            total_prizes_bp = total_prizes_bp
                .checked_add(prize_bp)
                .ok_or(Error::InvalidParameters)?;
        }
        if total_prizes_bp != 10000 {
            return Err(Error::InvalidParameters);
        }

        if config.protocol_fee_bp > 10000 {
            return Err(Error::InvalidParameters);
        }
        if config.protocol_fee_bp > 0 && config.treasury_address.is_none() {
            return Err(Error::InvalidParameters);
        }

if config.randomness_source == RandomnessSource::External {
            match config.oracle_address {
                None => return Err(Error::InvalidParameters),
                Some(ref addr) if *addr == env.current_contract_address() => {
                    return Err(Error::InvalidParameters);
                }
                Some(_) => {}
            }
        }

        // Quorum validation: k must be > 0, oracles must be non-empty, and
        // oracle_address must not be set (oracles are embedded in the enum).
        if let RandomnessSource::Quorum(QuorumConfig { k, oracles }) = &config.randomness_source {
            if *k == 0 || *k > oracles.len() as u32 {
                return Err(Error::InvalidParameters);
            }
            if oracles.len() > 10 {
                return Err(Error::InvalidParameters);
            }
            // Self-check: none of the oracles may be the raffle contract itself.
            for i in 0..oracles.len() {
                if let Some(addr) = oracles.get(i) {
                    if addr == env.current_contract_address() {
                        return Err(Error::InvalidParameters);
                    }
                }
            }
            // Quorum mode must not also set a single oracle_address.
            if config.oracle_address.is_some() {
                return Err(Error::InvalidParameters);
            }
        }

        if config.randomness_source != RandomnessSource::External
            && config.randomness_source
                != RandomnessSource::Quorum(QuorumConfig {
                    k: 1,
                    oracles: Vec::new(&env),
                })
            && config.oracle_address.is_some()
        {
            return Err(Error::InvalidParameters);
        }

        if config.metadata_hash == BytesN::from_array(&env, &[0u8; 32]) {
            return Err(Error::InvalidParameters);
        }

        // Validate that the payment_token is a valid token contract
        validate_token_address(&env, &config.payment_token)?;

        // The prize-token config override is not wired into initialization.
        let prize_token = config.payment_token.clone();

        // Resolve default values for fields that use 0 as "use default"
        let mut config = config.resolve_defaults();

        // `allow_multiple = false` is the legacy spelling of a one-ticket cap.
        // The explicit `max_tickets_per_address` cap supersedes it when set.
        if !config.allow_multiple && config.max_tickets_per_address == 0 {
            config.max_tickets_per_address = 1;
        }

        // #259: claim_lockup_seconds must be within [0, MAX_CLAIM_LOCKUP_SECONDS].
        let claim_lockup = config.claim_lockup_seconds.unwrap_or(DEFAULT_CLAIM_LOCKUP_SECONDS);
        if claim_lockup > MAX_CLAIM_LOCKUP_SECONDS {
            return Err(Error::InvalidParameters);
        }

        let claim_expiry = config.claim_expiry_seconds.unwrap_or(DEFAULT_CLAIM_EXPIRY_SECONDS);
        if claim_expiry < MIN_CLAIM_EXPIRY_SECONDS {
            return Err(Error::InvalidParameters);
        }
        if claim_expiry <= claim_lockup {
            return Err(Error::InvalidParameters);
        }

        // Swap deadline must be within [0, MAX_SWAP_DEADLINE_SECONDS].
        let swap_deadline = config.swap_deadline_seconds.unwrap_or(DEFAULT_SWAP_DEADLINE_SECONDS);
        if swap_deadline > MAX_SWAP_DEADLINE_SECONDS {
            return Err(Error::InvalidParameters);
        }

        // Validate early bird parameters
        if config.early_bird_ticket_percentage > 100 {
            return Err(Error::InvalidParameters);
        }
        if config.early_bird_ticket_percentage > 0 && config.early_bird_discount_bp > 10000 {
            return Err(Error::InvalidParameters);
        }

        let raffle = Raffle {
            creator: creator.clone(),
            description: config.description.clone(),
            end_time: config.end_time,
            no_deadline: config.no_deadline,
            max_tickets: config.max_tickets,
            max_tickets_per_tx: config.max_tickets_per_tx,
            max_tickets_per_address: config.max_tickets_per_address,
            min_tickets: config.min_tickets,
            allow_multiple: config.allow_multiple,
            ticket_price: config.ticket_price,
            payment_token: config.payment_token.clone(),
            prize_token: prize_token.clone(),
            prize_amount: config.prize_amount,
            prizes: config.prizes.clone(),
            tickets_sold: 0,
            status: RaffleStatus::PendingPrize,
            prize_deposited: false,
            winners: Vec::new(&env),
            randomness_source: config.randomness_source.clone(),
            oracle_address: config.oracle_address,
            protocol_fee_bp: config.protocol_fee_bp,
            treasury_address: config.treasury_address,
            swap_router: config.swap_router,
            tikka_token: config.tikka_token,
            finalized_at: None,
            claim_lockup_seconds: claim_lockup,
            claim_expiry_seconds: claim_expiry,
            swap_deadline_seconds: swap_deadline,
            ticket_sales_paused: false,
            early_bird_ticket_percentage: config.early_bird_ticket_percentage,
            early_bird_discount_bp: config.early_bird_discount_bp,
            metadata_hash: config.metadata_hash.clone(),
            unique_winners: config.unique_winners,
            bundles: config.bundles.clone(),
            nft_contract: config.nft_contract.clone(),
        };
        // Config validation above runs first so that a re-init with an
        // invalid config reports the actual config error rather than
        // `AlreadyInitialized`.
        if env.storage().instance().has(&DataKey::Raffle) {
            return Err(Error::AlreadyInitialized);
        }

        write_raffle(&env, &raffle);
        env.storage().instance().set(&DataKey::Factory, &factory);
        env.storage().instance().set(&DataKey::Admin, &admin);

        RaffleCreated {
            raffle_id: env.current_contract_address(),
            creator,
            end_time: config.end_time,
            max_tickets: config.max_tickets,
            ticket_price: config.ticket_price,
            payment_token: config.payment_token,
            prize_amount: config.prize_amount,
            prizes: config.prizes,
            description: config.description,
            randomness_source: config.randomness_source,
            metadata_hash: config.metadata_hash,
            unique_winners: config.unique_winners,
            claim_expiry_seconds: claim_expiry,
        }
        .publish(&env);

        let result = Ok(());
        #[cfg(any(test, feature = "testutils"))]
        assert_solvent_after_success(&env, &result);
        result
    }

    pub fn deposit_prize(env: Env) -> Result<(), Error> {
        let result = init::deposit_prize(env.clone());
        #[cfg(any(test, feature = "testutils"))]
        assert_solvent_after_success(&env, &result);
        result
    }

    pub fn buy_tickets(env: Env, buyer: Address, quantity: u32) -> Result<u32, Error> {
        let result = tickets::buy_tickets(env.clone(), buyer, quantity);
        #[cfg(any(test, feature = "testutils"))]
        assert_solvent_after_success(&env, &result);
        result
    }

    pub fn buy_tickets_for(
        env: Env,
        buyer: Address,
        recipient: Address,
        quantity: u32,
    ) -> Result<u32, Error> {
        let result = tickets::buy_tickets_for(env.clone(), buyer, recipient, quantity);
        #[cfg(any(test, feature = "testutils"))]
        assert_solvent_after_success(&env, &result);
        result
    }

    pub fn submit_commit(env: Env, ticket_id: u32, hash: BytesN<32>) -> Result<(), Error> {
        let result = tickets::submit_commit(env.clone(), ticket_id, hash);
        #[cfg(any(test, feature = "testutils"))]
        assert_solvent_after_success(&env, &result);
        result
    }

    pub fn finalize_raffle(env: Env) -> Result<(), Error> {
        let result = draw::finalize_raffle(env.clone());
        #[cfg(any(test, feature = "testutils"))]
        assert_solvent_after_success(&env, &result);
        result
    }

    pub fn provide_randomness(
        env: Env,
        random_seed: u64,
        public_key: BytesN<32>,
        proof: BytesN<64>,
        request_id: u64,
    ) -> Result<Address, Error> {
        let result = draw::provide_randomness(env.clone(), random_seed, public_key, proof, request_id);
        #[cfg(any(test, feature = "testutils"))]
        assert_solvent_after_success(&env, &result);
        result
    }

    /// Phase 1 of the k-of-n quorum commit-reveal protocol (#986): record a
    /// blinded seed commitment from one of the registered oracles.
    ///
    /// `commit` must be `sha256(seed_be_bytes || oracle_address_xdr ||
    /// request_id_be_bytes)` so a later reveal can be verified on-chain and
    /// bound to this oracle and this randomness request.  No seed material is
    /// written in this phase, so later submitters learn nothing about earlier
    /// ones.
    pub fn provide_quorum_commit(
        env: Env,
        oracle: Address,
        commit: BytesN<32>,
        request_id: u64,
    ) -> Result<(), Error> {
        draw::provide_quorum_commit(env, oracle, commit, request_id)
    }

    /// Phase 2 of the k-of-n quorum commit-reveal protocol (#986): open a
    /// previously committed seed.
    ///
    /// The reveal is rejected unless the oracle committed in phase 1, the
    /// recorded commitment hashes to `sha256(seed || oracle || request_id)`,
    /// and at least `k` commitments are already on record.  The raffle is
    /// finalized on the k-th valid reveal — never on a commit alone.
    pub fn provide_quorum_randomness(
        env: Env,
        oracle: Address,
        random_seed: u64,
        request_id: u64,
    ) -> Result<(), Error> {
        let result = draw::provide_quorum_randomness(env.clone(), oracle, random_seed, request_id);
        #[cfg(any(test, feature = "testutils"))]
        assert_solvent_after_success(&env, &result);
        result
    }

    /// Open a ticket-holder commit made while the raffle was `Active` for the
    /// `CommitReveal` randomness mode (#989).
    ///
    /// The seed of a `CommitReveal` draw is derived only from revealed
    /// pre-images; commits that are never revealed cannot influence it.
    pub fn reveal_commit(env: Env, ticket_id: u32, preimage: BytesN<32>) -> Result<(), Error> {
        draw::reveal_commit(env, ticket_id, preimage)
    }

    pub fn trigger_randomness_fallback(
        env: Env,
        caller: Address,
        do_refund: bool,
    ) -> Result<(), Error> {
        let result = draw::trigger_randomness_fallback(env.clone(), caller, do_refund);
        #[cfg(any(test, feature = "testutils"))]
        assert_solvent_after_success(&env, &result);
        result
    }

    pub fn claim_prize(env: Env, winner: Address, tier_index: u32) -> Result<i128, Error> {
        let result = claim::claim_prize(env.clone(), winner, tier_index);
        #[cfg(any(test, feature = "testutils"))]
        assert_solvent_after_success(&env, &result);
        result
    }

    /// Permissionless sweep of unclaimed prizes to treasury after `claim_expiry_seconds`
    /// has elapsed since finalization.  Processes at most `limit` tiers starting at
    /// `start_index`.  Returns the number of prizes swept in this call.
    pub fn sweep_unclaimed(
        env: Env,
        start_index: u32,
        limit: u32,
    ) -> Result<u32, Error> {
        let result = crate::claim::sweep_unclaimed(env.clone(), start_index, limit);
        #[cfg(any(test, feature = "testutils"))]
        assert_solvent_after_success(&env, &result);
        result
    }

    pub fn withdraw_fees(env: Env, recipient: Address, amount: i128) -> Result<(), Error> {
        let result = admin::withdraw_fees(env.clone(), recipient, amount);
        #[cfg(any(test, feature = "testutils"))]
        assert_solvent_after_success(&env, &result);
        result
    }

    pub fn get_accumulated_fees(env: Env) -> i128 {
        views::get_accumulated_fees(env)
    }

    /// Aggregate dashboard view returning key raffle metrics in a single call.
    ///
    /// See [`views::get_stats`] for full documentation.
    pub fn get_stats(env: Env) -> Result<RaffleStats, Error> {
        views::get_stats(env)
    }

    pub fn cancel_raffle(env: Env, reason: CancelReason) -> Result<(), Error> {
        let result = admin::cancel_raffle(env.clone(), reason);
        #[cfg(any(test, feature = "testutils"))]
        assert_solvent_after_success(&env, &result);
        result
    }

    /// Executes a previously scheduled admin cancellation (#406).
    ///
    /// Only succeeds once the timelock set by `cancel_raffle` has elapsed.
    /// Calling it earlier returns `CancelTimelockActive`; calling it with no
    /// pending schedule returns `CancelNotScheduled`.
    pub fn execute_admin_cancel(env: Env) -> Result<(), Error> {
        admin::execute_admin_cancel(env)
    }

    /// Returns the timestamp at which a scheduled admin cancel becomes
    /// executable, or `None` if no cancel is currently scheduled (#406).
    pub fn get_pending_cancel(env: Env) -> Option<u64> {
        views::get_pending_cancel(env)
    }

    pub fn refund_prize(env: Env) -> Result<(), Error> {
        let result = claim::refund_prize(env.clone());
        #[cfg(any(test, feature = "testutils"))]
        assert_solvent_after_success(&env, &result);
        result
    }

    pub fn emergency_withdraw(env: Env, caller: Address) -> Result<(), Error> {
        let result = admin::emergency_withdraw(env.clone(), caller);
        #[cfg(any(test, feature = "testutils"))]
        assert_solvent_after_success(&env, &result);
        result
    }

    pub fn refund_ticket(env: Env, ticket_id: u32) -> Result<i128, Error> {
        claim::refund_ticket(env, ticket_id)
    }

    pub fn batch_refund_tickets(
        env: Env,
        caller: Address,
        ticket_ids: Vec<u32>,
    ) -> Result<i128, Error> {
        claim::batch_refund_tickets(env, caller, ticket_ids)
    }

    pub fn get_raffle(env: Env) -> Result<Raffle, Error> {
        views::get_raffle(env)
    }

    pub fn get_fairness_data(env: Env) -> Result<FairnessData, Error> {
        views::get_fairness_data(env)
    }

    /// Return a complete attestation package for third-party draw verification.
    ///
    /// Combines fairness data, metadata hash, winner addresses, winning ticket IDs,
    /// randomness source, and a hash of the effective raffle configuration into a
    /// single response. A verifier needs only this one call to obtain everything
    /// required to independently re-derive the winners.
    ///
    /// Only available in `Finalized` or `Claimed` states; returns `InvalidStatus`
    /// otherwise.
    ///
    /// See [`docs/RANDOMNESS.md`] for the verification procedure.
    pub fn get_draw_attestation(
        env: Env,
    ) -> Result<attestation::DrawAttestation, Error> {
        attestation::get_draw_attestation(&env)
    }

    /// Return all ticket IDs owned by `owner`.
    ///
    /// Uses the `OwnerTickets` index maintained during `buy_tickets` for an
    /// O(1) read.  Falls back to an empty Vec when the address has never
    /// purchased a ticket.
    pub fn get_my_tickets(env: Env, owner: Address) -> Vec<u32> {
        views::get_my_tickets(env, owner)
    }

    pub fn wipe_storage(env: Env) -> Result<(), Error> {
        let result = admin::wipe_storage(env.clone());
        #[cfg(any(test, feature = "testutils"))]
        assert_solvent_after_success(&env, &result);
        result
    }

    pub fn pause(env: Env) -> Result<(), Error> {
        let result = admin::pause(env.clone());
        #[cfg(any(test, feature = "testutils"))]
        assert_solvent_after_success(&env, &result);
        result
    }

    pub fn unpause(env: Env) -> Result<(), Error> {
        let result = admin::unpause(env.clone());
        #[cfg(any(test, feature = "testutils"))]
        assert_solvent_after_success(&env, &result);
        result
    }

    pub fn is_paused(env: Env) -> bool {
        views::is_paused(env)
    }

    pub fn pause_ticket_sales(env: Env, caller: Address) -> Result<(), Error> {
        let result = admin::pause_ticket_sales(env.clone(), caller);
        #[cfg(any(test, feature = "testutils"))]
        assert_solvent_after_success(&env, &result);
        result
    }

    pub fn resume_ticket_sales(env: Env, caller: Address) -> Result<(), Error> {
        let result = admin::resume_ticket_sales(env.clone(), caller);
        #[cfg(any(test, feature = "testutils"))]
        assert_solvent_after_success(&env, &result);
        result
    }

    pub fn is_ticket_sales_paused(env: Env) -> bool {
        views::is_ticket_sales_paused(env)
    }

    pub fn get_remaining_ticket_allowance(
        env: Env,
        owner: Address,
    ) -> Result<u32, Error> {
        views::get_remaining_ticket_allowance(env, owner)
    }

    /// Quote the exact cost of buying `quantity` tickets including early-bird
    /// discounts and protocol fees.
    ///
    /// Read-only — does not mutate state, does not require auth, does not
    /// check raffle status, pausing, or availability.  Returns a
    /// [`BuyQuote`] with the full pricing breakdown.  Uses the same
    /// internal helper as `buy_tickets` so quote and execution cannot
    /// diverge.
    pub fn preview_buy(env: Env, quantity: u32) -> Result<BuyQuote, Error> {
        views::preview_buy(env, quantity)
    }

    /// Sweep tokens that were accidentally sent to this contract.
    /// Configured payment and prize tokens can only be swept to the extent
    /// that all outstanding refunds, fees, and prize claims remain covered.
    pub fn rescue_tokens(
        env: Env,
        token: Address,
        recipient: Address,
        amount: i128,
    ) -> Result<(), Error> {
        let result = admin::rescue_tokens(env.clone(), token, recipient, amount);
        #[cfg(any(test, feature = "testutils"))]
        assert_solvent_after_success(&env, &result);
        result
    }

    /// Sweep residual payment-token balance to the treasury after the raffle is
    /// fully settled (`Claimed` or `Cancelled` with no outstanding prize or
    /// ticket-refund entitlements).
    ///
    /// See also: [`docs/EVENTS.md`](../../../docs/EVENTS.md) — `DustSwept`.
    pub fn sweep_dust(env: Env) -> Result<(), Error> {
        let result = self::admin::sweep_dust(env.clone());
        #[cfg(any(test, feature = "testutils"))]
        assert_solvent_after_success(&env, &result);
        result
    }

    pub fn update_oracle_address(env: Env, new_oracle: Address) -> Result<(), Error> {
        let result = admin::update_oracle_address(env.clone(), new_oracle);
        #[cfg(any(test, feature = "testutils"))]
        assert_solvent_after_success(&env, &result);
        result
    }

    pub fn set_protocol_fee_bp(env: Env, new_fee_bp: u32) -> Result<(), Error> {
        let result = admin::set_protocol_fee_bp(env.clone(), new_fee_bp);
        #[cfg(any(test, feature = "testutils"))]
        assert_solvent_after_success(&env, &result);
        result
    }

    /// Rotate the instance admin address. Invoked cross-contract by the
    /// factory's `sync_admin` entrypoint.
    pub fn set_admin(env: Env, new_admin: Address) -> Result<(), Error> {
        admin::set_admin(env, new_admin)
    }

    pub fn set_swap_deadline(env: Env, new_deadline_seconds: u64) -> Result<(), Error> {
        let result = admin::set_swap_deadline(env.clone(), new_deadline_seconds);
        #[cfg(any(test, feature = "testutils"))]
        assert_solvent_after_success(&env, &result);
        result
    }

    pub fn update_metadata_hash(env: Env, new_hash: BytesN<32>) -> Result<(), Error> {
        admin::update_metadata_hash(env, new_hash)
    }

    /// Permissionless entrypoint — anyone may call this to prevent a raffle
    /// from being archived by Soroban's TTL expiry.
    ///
    /// This entrypoint is currently unimplemented and returns
    /// [`Error::InvalidParameters`]. It does not bump any TTLs.
    pub fn extend_ttl(env: Env) -> Result<(), Error> {
        let _raffle = read_raffle(&env)?;
        Err(Error::InvalidParameters)
    }
}

#[cfg(test)]
mod test;
#[cfg(test)]
mod tests;