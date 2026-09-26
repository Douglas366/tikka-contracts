//! Shared fixtures, helpers, and mock contracts for raffle-instance integration tests.

#[cfg(test)]

extern crate std;
use crate::*;
use soroban_sdk::{
	testutils::{Events, Ledger},
	token::StellarAssetClient,
	Address, BytesN, Env, String,
};

/// Assert the drawing lock was cleared from instance storage.
pub(crate) fn assert_drawing_lock_cleared(env: &Env, contract_id: &Address) {
	let is_set: bool = env.as_contract(contract_id, || {
		env.storage()
			.instance()
			.get(&crate::DataKey::DrawingLock)
			.unwrap_or(false)
	});
	assert!(!is_set, "DrawingLock must be cleared");
}

/// Register a fresh SAC and return its address plus its mint client.
pub fn create_token<'a>(env: &'a Env, admin: &Address) -> (Address, StellarAssetClient<'a>) {
	let payment_token = env
		.register_stellar_asset_contract_v2(admin.clone())
		.address();
	(
		payment_token.clone(),
		StellarAssetClient::new(env, &payment_token),
	)
}

/// `sha256` of a 32-byte pre-image — the commitment `submit_commit` stores.
pub fn sha256_bytes32(env: &Env, preimage: &[u8; 32]) -> BytesN<32> {
	env.crypto()
		.sha256(&soroban_sdk::Bytes::from_array(env, preimage))
		.into()
}

/// Assert the raffle's stored metadata hash matches the expected bytes.
#[allow(dead_code)] // referenced by tests/init.rs, which is not part of this build
pub fn assert_metadata_hash(client: &ContractClient<'_>, expected: &BytesN<32>) {
	let raffle = client.get_raffle();
	assert_eq!(raffle.metadata_hash, *expected);
}

/// Generate an address usable as the calling factory in `client.init`.
pub fn creator_factory_addr(env: &Env) -> Address {
	Address::generate(env)
}

/// Set up a quorum raffle in Drawing state with tickets sold and randomness requested.
pub fn setup_quorum_drawing_raffle<'a>(
	env: &'a Env,
	k: u32,
	oracles: &[Address],
) -> (ContractClient<'a>, Address, Address, u64) {
	// A previous draw in the same test env may have advanced the ledger past
	// `end_time`; rewind so `init` sees a future deadline.
	env.ledger().set_timestamp(0);

	let contract_id = env.register(Contract, ());
	let client = ContractClient::new(env, &contract_id);
	let factory = env.register(MockFactory, ());
	let admin = Address::generate(env);
	let creator = Address::generate(env);

	let token_admin = Address::generate(env);
	let (token_addr, token_mint) = create_token(env, &token_admin);
	token_mint.mint(&creator, &1_000_000);

	let mut oracle_vec = Vec::new(env);
	for oracle in oracles {
		oracle_vec.push_back(oracle.clone());
	}

	let config = RaffleConfig {
		description: String::from_str(env, "quorum raffle"),
		end_time: 1_000,
		no_deadline: false,
		max_tickets: 5,
		max_tickets_per_tx: 5,
		max_tickets_per_address: 0,
		min_tickets: 1,
		allow_multiple: true,
		ticket_price: MIN_TICKET_PRICE,
		payment_token: token_addr,
		prize_amount: MIN_TICKET_PRICE * 5,
		prizes: soroban_sdk::vec![env, 10000u32],
		randomness_source: RandomnessSource::Quorum(QuorumConfig {
			k,
			oracles: oracle_vec,
		}),
		oracle_address: None,
		protocol_fee_bp: 0,
		treasury_address: None,
		swap_router: None,
		tikka_token: None,
		metadata_hash: BytesN::from_array(env, &[55u8; 32]),
		claim_lockup_seconds: None,
		claim_expiry_seconds: None,
		swap_deadline_seconds: None,
		early_bird_ticket_percentage: 0,
		early_bird_discount_bp: 0,
		category: None,
		unique_winners: false,
		bundles: Vec::new(env),
		prize_token: None,
		nft_contract: None,
	};

	client.init(&factory, &admin, &creator, &config);
	client.deposit_prize();
	client.buy_tickets(&creator, &3);
	env.ledger().set_timestamp(1_000);
	client.finalize_raffle();

	let request_id: u64 = env.as_contract(&contract_id, || {
		env.storage()
			.instance()
			.get(&DataKey::RandomnessRequestId)
			.unwrap_or(0)
	});

	(client, contract_id, creator, request_id)
}

pub mod budget;
pub mod fairness;
pub mod draw;
pub mod invariants;
pub mod ttl;
