// Copyright 2020 Parity Technologies (UK) Ltd.
// This file is part of Substrate.

// Substrate is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Substrate is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Substrate.  If not, see <http://www.gnu.org/licenses/>.

//! The Bet: A simple Bet of a runtime module demonstrating
//! concepts, APIs and structures common to most runtime modules.

// Ensure we're `no_std` when compiling for Wasm.
#![cfg_attr(not(feature = "std"), no_std)]

use sp_runtime::{traits::{One, Zero, Bounded}};
use frame_support::{
	decl_event, decl_module, decl_storage, Parameter,
	traits::{
		OnFreeBalanceZero, Currency, LockableCurrency, WithdrawReason, WithdrawReasons,
		LockIdentifier
	}
};
use frame_system::{self as system, ensure_signed};
use codec::{Encode, Decode};

/// Trait for getting a price.
pub trait FetchPrice<Balance> {
	/// Fetch the price.
	fn fetch_price() -> Balance;
}

const MODULE_ID: LockIdentifier = *b"py/fun__";

type BalanceOf<T> = <<T as Trait>::Currency as Currency<<T as system::Trait>::AccountId>>::Balance;

/// Our module's configuration trait.
pub trait Trait: system::Trait {
	/// The overarching event type.
	type Event: From<Event<Self>> + Into<<Self as system::Trait>::Event>;

	/// Get the amount of tokens worth 1 Euro.
	type OneEuro: FetchPrice<BalanceOf<Self>>;

	/// The currency type.
	type Currency: Currency<Self::AccountId> + LockableCurrency<Self::AccountId, Moment=Self::BlockNumber>;
}

// Periods
// Block
// 0 1 2 3 4 5 6 7 8 9
// Period
// 0 0 0 1 1 1 2 2 2 3
//


#[derive(Encode, Decode, Clone, Eq, PartialEq)]
#[cfg_attr(feature = "std", derive(Debug))]
pub enum State<BlockNumber: Parameter> {
	Idle,
	BeganAt(BlockNumber),
	EndingAt(BlockNumber),
}
impl<BlockNumber: Parameter> Default for State<BlockNumber> {
	fn default() -> Self {
		State::Idle
	}
}

#[derive(Copy, Clone, PartialEq, Debug)]
enum ConsolidatedState {
	Idle,
	AboutToBegin,
	JustBegan,
	AboutToEnd,
}

#[derive(Copy, Clone, PartialEq, Debug)]
enum BetResult<Balance> {
	Success(Balance),
	Wipeout(Balance),
}

#[derive(Encode, Decode, Clone, Eq, PartialEq, Default)]
#[cfg_attr(feature = "std", derive(Debug))]
pub struct Betting<BlockNumber: Parameter, Balance: Parameter> {
	/// Our current betting state.
	state: State<BlockNumber>,

	/// The block that we are locked until.
	locked_until: Option<BlockNumber>,

	/// The balance with which we are betting.
	balance: Balance,
}

decl_storage! {
	trait Store for Module<T: Trait> as Bet {
		/// Period in which betting happens, measured in blocks.
		Period get(fn period) config(): T::BlockNumber = 1000.into();

		/// Factor controlling the attenuation speed of the target when missed.
		/// The price is reduced by a factor of one divided by this. It *must* be greater
		/// than one.
		TargetAttenuation get(fn target_attenuation) config(): BalanceOf<T>;

		/// The number of times to sample the spot price per period in order to determine the
		/// average price.
		Samples get(fn samples) config(): u32;

		/// The target price to beat.
		Target get(fn target) config(): BalanceOf<T>;

		/// Index of current period.
		Index get(fn index): T::BlockNumber;

		/// Betting information.
		Bets get(fn bets): map T::AccountId => Betting<T::BlockNumber, BalanceOf<T>>;

		/// This period's prices.
		Prices get(fn prices): Vec<BalanceOf<T>>;

		/// The pot.
		Pot get(fn pot): BalanceOf<T>;

		/// The cumulative amount that is staked for reward or wipeout at the end of the current index.
		Total get(fn total): BalanceOf<T>;

		/// The cumulative amount that will become additionally staked at the next index.
		Incoming get(fn incoming): BalanceOf<T>;

		/// The cumulative amount that will become unstaked at the next index iff it isn't a wipeout.
		Outgoing get(fn outgoing): BalanceOf<T>;

		/// Payout history. Some is when there's a payout (the first parameter is the total amount
		/// that was at stake at the point of payout, the second was the pot). None is when
		/// it's a wipeout.
		Payouts get(fn payouts): map T::BlockNumber => Option<(BalanceOf<T>, BalanceOf<T>)>;
	}
}

decl_event!(
	pub enum Event<T> where Balance = BalanceOf<T> {
		Dummy(Balance),
	}
);

/*
Example 1: Bet for a single period; collect-after-unlock.
				[p=0, None, None; "NULL"]
bet
				[p=0, Some(1), None; "UNLOCKED"]
period-end
				[p=1, Some(1), None; "BETTING"]
unbet
				[p=1, Some(1), Some(2); "BETTING"]
period-end
				[p=2, Some(1), Some(2); "LOCKED"]
period-end
				[p=3, Some(1), Some(2); "UNLOCKED"]
collect
				[p=4, None, None; "NULL"]

Example 2: Bet followed immediately be unbet is a no-op
				[p=0, None, None; "NULL"]
bet
				[p=0, Some(1), None; "UNLOCKED"]
unbet
				[p=0, None, None; "NULL"]
period-end
				[p=1, None, None; "NULL"]

Example 3: Double-bet/unbet is no-op
				[p=0, None, None; "NULL"]
bet
				[p=0, Some(1), None; "UNLOCKED"]
bet (no-op)
				[p=0, Some(1), None; "UNLOCKED"]
period-end
				[p=1, Some(1), None; "BETTING"]
bet (no-op)
				[p=1, Some(1), None; "BETTING"]
unbet
				[p=1, Some(1), Some(2); "BETTING"]
unbet (no-op)
				[p=1, Some(1), Some(2); "BETTING"]

Example 4: Double-bet/unbet is no-op
				[p=0, None, None; "NULL"]
bet
				[p=0, w=[], t=1, Some(1), None; "UNLOCKED"]
period-end
				[p=1, w=[N], t=0, Some(1), None; "BETTING"]
period-end
				[p=2, w=[N, 0], t=0, Some(1), None; "BETTING"]
unbet
				[p=2, w=[N, 0], t=0, Some(1), Some(3); "BETTING"]
bet
				[p=2, w=[N, 0], t=0, Some(1), None; "UNLOCKED"]
*/

// The module declaration.
decl_module! {
	// Simple declaration of the `Module` type. Lets the macro know what its working on.
	pub struct Module<T: Trait> for enum Call where origin: T::Origin {
		fn deposit_event() = default;

		/// Add the sender to the betting system. At the next period they will be betting
		/// that the price will go up and their funds locked for at least two periods. If they
		/// are currently not active, but were, then it will issue any payouts.
		fn bet(origin) {
			let sender = ensure_signed(origin)?;
			let current = Self::index();
			let next = current + One::one();

			let balance_at_stake_is_zero = <Bets<T>>::mutate(&sender, |b| {
				let cs = Self::consolidate(&current, &sender, b);

				// We are now guaranteed that b.state will be one of:
				// - Idle
				// - BeganAt(current)
				// - EndingAt(next)

				// Bets(sender) may no longer exist now (if our history implied we got wiped
				// out; check this and early-exit if so):
				if b.balance.is_zero() && cs != ConsolidatedState::Idle {
					return true;
				}

				match cs {
					ConsolidatedState::Idle => {
						b.state = State::BeganAt(next);
						b.balance = T::Currency::free_balance(&sender);
						<Incoming<T>>::mutate(|total| *total += b.balance);
					}
					ConsolidatedState::AboutToBegin | ConsolidatedState::JustBegan => {
						// Already betting. Nothing to do; bail to avoid erroneously accumulating balance.
						return b.balance.is_zero()
					}
					ConsolidatedState::AboutToEnd => {
						// Scheduled to end exactly when we're meant to start again. Current period is still to
						// be accounted for, so we reset to BeginAt the current. We can't update the balance to
						// `account_balance` since it would invalidate the current period's win calculation;
						// instead we use the old betted balance.
						b.state = State::BeganAt(current);
						<Outgoing<T>>::mutate(|total| *total -= b.balance);
					}
				};

				b.balance.is_zero()
			});
			
			// We've been wiped out: kill entry.
			if balance_at_stake_is_zero {
				<Bets<T>>::remove(&sender);
				T::Currency::remove_lock(MODULE_ID, &sender);
			} else {
				T::Currency::set_lock(
					MODULE_ID,
					&sender,
					<BalanceOf<T>>::max_value(),
					T::BlockNumber::max_value(),
					WithdrawReasons::except(WithdrawReason::TransactionPayment),
				);
			}
//			println!("{:?}", <Bets<T>>::get(&sender));
		}

		/// Remove the sender from the betting system. At the next period they will no
		/// longer be betting that the price will go up and their funds will be locked
		/// for one further period.
		fn unbet(origin) {
			let sender = ensure_signed(origin)?;

			let balance_at_stake_is_zero = <Bets<T>>::mutate(&sender, |b| {
				let cs = Self::consolidate(&Self::index(), &sender, b);
//				println!("unbet(): CONS {:?}", cs);

				// We are now guaranteed that b.state will be one of:
				// - Idle
				// - BeganAt(next)
				// - BeganAt(current)
				// - EndingAt(next)

				// Bets(sender) may no longer exist now (if our history implied we got wiped
				// out; check this and early-exit if so):
				if b.balance.is_zero() {
					return true;
				}

				match cs {
					ConsolidatedState::JustBegan => {
						let next = Self::index() + One::one();
						b.state = State::EndingAt(next);
						b.locked_until = Some(next + One::one());
//						println!("JUST BEGAN {:?} {:?}", b.balance, Self::total());
						<Outgoing<T>>::mutate(|total| *total += b.balance)
					}
					ConsolidatedState::AboutToBegin => {
						b.state = State::Idle;
//						println!("ABOUT TO BEGIN {:?} {:?}", b.balance, Self::total());
						<Incoming<T>>::mutate(|total| *total -= b.balance)
					}
					_ => {}
				};
				false
			});

			// We've been wiped out: kill entry.
			if balance_at_stake_is_zero {
				<Bets<T>>::remove(&sender);
				T::Currency::remove_lock(MODULE_ID, &sender);
			}
		}

		/// Withdraw from the system in general. You must be past the lock period for
		/// this not to be a no-op.
		fn collect(origin) {
			let sender = ensure_signed(origin)?;

			let is_unlocked = <Bets<T>>::mutate(&sender, |b| {
				Self::consolidate(&Self::index(), &sender, b);
				b.state == State::Idle && b.locked_until.map_or(true, |l| l <= Self::index())
			});

			if is_unlocked {
				<Bets<T>>::remove(&sender);
				T::Currency::remove_lock(MODULE_ID, &sender);
			}
		}

		// The signature could also look like: `fn on_finalize()`
		fn on_finalize(n: T::BlockNumber) {
			let samples = Self::samples();
			let p = Self::period();

			let mp = p / samples.into();
			let ph = p - One::one() - n % p;

			// For samples = 3, period = 7, mp = 2
			// n:   0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6
			// n%p: 0 1 2 3 4 5 6 0 1 2 3 4 5 6 0 1 2
			// ph:  6 5 4 3 2 1 0 6 5 4 3 2 1 0 6 5 4
			//          +   +   *     +   +   *     +
			// [+: take sample, *: take sample, end period]

			if (ph % mp).is_zero() && ph / mp < samples.into() {
				// end of segment
				let one_euro = T::OneEuro::fetch_price();

				<Prices<T>>::mutate(|prices| prices.push(one_euro));

				if ph.is_zero() {
					// end of period
//					println!("Ending period: {:?} block #{:?}", Self::index(), n);

					let prices = <Prices<T>>::take();
					let total = <Total<T>>::get();

					if !total.is_zero() {
						let mean = prices.iter().fold(BalanceOf::<T>::default(), |sum, &item| sum + item) / samples.into();

//						println!("prices {:?} mean {:?} target {:?}", prices, mean, Self::target());
						if mean < Self::target() {
							// payout
							let pot = <Pot<T>>::take();
							<Target<T>>::put(mean);
							let accrued_outgoing = <Outgoing<T>>::take() * (total + pot) / total;
							<Total<T>>::put(total + pot + <Incoming<T>>::take() - accrued_outgoing);
							// This is where the total should be expanded for contiguous betters.
							<Payouts<T>>::insert(Self::index(), (total, pot));
						} else {
							// wipeout
							<Target<T>>::mutate(|p| *p = *p / Self::target_attenuation() * (Self::target_attenuation() + One::one()));
							<Outgoing<T>>::kill();
							<Total<T>>::put(<Incoming<T>>::take());
						}

//						println!("Payout: {:?}", Self::payouts(Self::index()));
					} else {
//						println!("No payout - no users");
						<Total<T>>::put(<Incoming<T>>::take());
					}

					<Index<T>>::mutate(|i| *i += One::one());
//					println!("Next period: {:?}", Self::index());
				}
			}
		}
	}
}

// TODO: if <Bets<T>>::exists(who) implies account balance locked.

impl<T: Trait> Module<T> {
	/// Contibute some funds to the pot. (It is assumed that the funds are burned elsewhere in the system.)
	pub fn contribute(value: BalanceOf<T>) {
		<Pot<T>>::mutate(|p| *p += value);
	}

	/// Consolidates the `betting` state of `who` into one of `Idle, BeganAt(Self::index()) and EndingAt(Self::index + 1)`
	/// Calling this could delete the relevant entry in `Bets`.
	fn consolidate(now: &T::BlockNumber, who: &T::AccountId, betting: &mut Betting<T::BlockNumber, BalanceOf<T>>) -> ConsolidatedState {
//		println!("consolidate CONS {:?} now: {}", betting, now);
		let (new_balance, result) = match betting.state.clone() {
			State::BeganAt(n) if n < *now => {
				// calculate and impose new balance implied by [n ... now)
				betting.state = State::BeganAt(*now);
				match Self::calculate_new_balance(betting.balance, n, *now) {
					BetResult::Success(b) => (b, ConsolidatedState::JustBegan),
					BetResult::Wipeout(b) => { betting.locked_until = None; (b, ConsolidatedState::Idle) }
				}
			}
			State::EndingAt(n) if n <= *now => {
				// calculate new balance implied by n
				betting.state = State::Idle;
				(
					match Self::calculate_new_balance(betting.balance, n - One::one(), n) {
						BetResult::Success(b) => b,
						BetResult::Wipeout(b) => { betting.locked_until = None; b }
					},
					ConsolidatedState::Idle
				)
			}
			State::BeganAt(n) if n == *now => return ConsolidatedState::JustBegan,
			State::BeganAt(_) /*if _ > now*/ => return ConsolidatedState::AboutToBegin,
			State::EndingAt(_) => return ConsolidatedState::AboutToEnd,
			State::Idle => return ConsolidatedState::Idle,
		};

		if betting.balance < new_balance {
			// TODO: SHAWN CHECK DEPOSIT_CREATING WORKS HERE
			T::Currency::deposit_creating(who, new_balance - betting.balance);
		} else {
			// this action might delete our entry in Bets (if free_balance is reduced to zero).
			// it's ok though, since mutate will write it back out with expected values.
			// TODO: SHAWN CHECK SLASH WORKS HERE
			let _ = T::Currency::slash(who, betting.balance - new_balance);
		}

		betting.balance = new_balance;

//		println!("Consolidated: {:?}", betting);
		result
	}

	/// Returns the new balance (i.e. old plus the payout reward); will be zero if there was a wipeout.
	fn calculate_new_balance(
		balance: BalanceOf<T>,
		begin: T::BlockNumber,
		end: T::BlockNumber
	) -> BetResult<BalanceOf<T>> {
//		println!("Calculating new... {:?} {:?} {:?}", balance, begin, end);
		if balance.is_zero() {
			// nothing to be done here
			return BetResult::Wipeout(balance)
		}
		// pay out (or wipeout) coming...
		let mut b = begin;
		let mut new_balance = balance;
		while b < end {
			// accumulate winnings
			match Self::payouts(b) {
				Some((total_stake, pot)) => {
					// A(nother) win! Accumulate.
					// TODO: check for overflow (we're assuming 32-bits at the upper end here).
					// See #935.
					let payout = ((balance << 32) / total_stake * pot) >> 32;
//					println!("Payout: {:?} from pot of {:?} (total staked was {:?})", payout, pot, total_stake);
					new_balance += payout;
					// This is where the total should be expanded for contiguous betters.
				}
				None => {
					// wipeout.
					return BetResult::Wipeout(new_balance >> 1)
				}
			}
			b += One::one();
		}
		BetResult::Success(new_balance)
	}
}

impl<T: Trait> OnFreeBalanceZero<T::AccountId> for Module<T> {
	fn on_free_balance_zero(who: &T::AccountId) {
		<Bets<T>>::remove(who);
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	use ::std::cell::Cell;
	use sp_core::H256;
	// The testing primitives are very useful for avoiding having to work with signatures
	// or public keys. `u64` is used as the `AccountId` and no `Signature`s are requried.
	use sp_runtime::{
		Perbill,
		traits::{BlakeTwo256, OnInitialize, OnFinalize, IdentityLookup},
		testing::Header,
	};
	use frame_support::{impl_outer_origin, assert_ok, parameter_types, weights::Weight};


	thread_local! { static ONE_EURO: Cell<u64> = Cell::new(100); }
	pub struct StaticOneEuro;
	impl FetchPrice<u64> for StaticOneEuro {
		fn fetch_price() -> u64 {
			ONE_EURO.with(|o| o.get())
		}
	}
	fn set_price(p: u64) {
		ONE_EURO.with(|o| o.set(p));
	}

	impl_outer_origin! {
		pub enum Origin for Test {}
	}

	// For testing the module, we construct most of a mock runtime. This means
	// first constructing a configuration type (`Test`) which `impl`s each of the
	// configuration traits of modules we want to use.
	#[derive(Clone, Eq, PartialEq)]
	pub struct Test;
	parameter_types! {
		pub const BlockHashCount: u64 = 250;
		pub const MaximumBlockWeight: Weight = 1024;
		pub const MaximumBlockLength: u32 = 2 * 1024;
		pub const AvailableBlockRatio: Perbill = Perbill::one();
	}
	impl frame_system::Trait for Test {
		type Origin = Origin;
		type Index = u64;
		type BlockNumber = u64;
		type Call = ();
		type Hash = H256;
		type Hashing = BlakeTwo256;
		type AccountId = u64;
		type Lookup = IdentityLookup<Self::AccountId>;
		type Header = Header;
		type Event = ();
		type BlockHashCount = BlockHashCount;
		type MaximumBlockWeight = MaximumBlockWeight;
		type MaximumBlockLength = MaximumBlockLength;
		type AvailableBlockRatio = AvailableBlockRatio;
		type Version = ();
		type ModuleToIndex = ();
	}
	parameter_types! {
		pub const ExistentialDeposit: u64 = 0;
		pub const TransferFee: u64 = 0;
		pub const CreationFee: u64 = 0;
	}
	impl pallet_balances::Trait for Test {
		type Balance = u64;
		type OnFreeBalanceZero = ();
		type OnNewAccount = ();
		type Event = ();
		type TransferPayment = ();
		type DustRemoval = ();
		type ExistentialDeposit = ExistentialDeposit;
		type TransferFee = TransferFee;
		type CreationFee = CreationFee;
	}
	impl Trait for Test {
		type Event = ();
		type OneEuro = StaticOneEuro;
		type Currency = Balances;
	}
	type System = system::Module<Test>;
	type Balances = pallet_balances::Module<Test>;
	type Bet = Module<Test>;

	// This function basically just builds a genesis storage key/value store according to
	// our desired mockup.
	fn new_test_ext() -> sp_io::TestExternalities {
		let mut t = system::GenesisConfig::default().build_storage::<Test>().unwrap();
		// We use default for brevity, but you can configure as desired if needed.
		pallet_balances::GenesisConfig::<Test>{
			balances: vec![(1, 10), (2, 20), (3, 30), (4, 40)],
			vesting: vec![],
			//reclaim_rebate: 0,		// TODO: remove when merge to master!
		}.assimilate_storage(&mut t).unwrap();
		GenesisConfig::<Test>{
			period: 5,
			samples: 2,
			target_attenuation: 10,
			target: 120,
		}.assimilate_storage(&mut t).unwrap();
		sp_io::TestExternalities::new(t)
	}

	fn account_is_liquid(who: &<Test as frame_system::Trait>::AccountId) -> bool {
		Balances::locks(who).is_empty()
	}

	// Run until a particular block.
	fn run_to_block(n: u64) {
		while System::block_number() < n {
			if System::block_number() > 1 {
				System::on_finalize(System::block_number());
				Bet::on_finalize(System::block_number());
			}
			System::set_block_number(System::block_number() + 1);
			System::on_initialize(System::block_number());
		}
	}

	// Run until the next bet index.
	fn run_to_next_index() {
		let i = Bet::index();
		while Bet::index() == i {
			System::on_finalize(System::block_number());
			Bet::on_finalize(System::block_number());
			System::set_block_number(System::block_number() + 1);
			System::on_initialize(System::block_number());
		}
	}

	#[test]
	fn config_works() {
		new_test_ext().execute_with(|| {
			assert_eq!(Bet::period(), 5);
			assert_eq!(Bet::samples(), 2);
			assert_eq!(Bet::target_attenuation(), 10);
			assert_eq!(Bet::target(), 120);
			assert_eq!(Bet::index(), 0);
			assert_eq!(Bet::bets(0), Betting::default());
			assert_eq!(Bet::prices(), vec![]);
			assert_eq!(Bet::pot(), 0);
			assert_eq!(Bet::total(), 0);
			assert_eq!(Bet::payouts(0), None);
		});
	}

	// For samples = 2, period = 5, mp = 2
	// n:   0 1 2 3 4 5 6 7 8 9 0 1 2 3 4
	// n%p: 0 1 2 3 4 0 1 2 3 4 0 1 2 3 4
	// ph:  4 3 2 1 0 4 3 2 1 0 4 3 2 1 0
	//          +   *     +   *     +   *
	// [+: take sample, *: take sample, end period]

	#[test]
	fn price_sampling_works() {
		new_test_ext().execute_with(|| {
			<Total<Test>>::put(1);

			run_to_block(1);
			assert_eq!(Bet::prices(), vec![]);
			set_price(120);

			run_to_block(2);
			assert_eq!(Bet::prices(), vec![]);
			set_price(80);
			// Take sample at the end of block 2: 80

			run_to_block(3);
			assert_eq!(Bet::prices(), vec![80]);
			set_price(140);

			run_to_block(4);
			assert_eq!(Bet::prices(), vec![80]);
			set_price(100);
			// Take sample at the end of block 4: 100

			run_to_block(5);
			// Target set: Average of 80 and 100 is 90
			assert_eq!(Bet::target(), 90);
			assert_eq!(Bet::payouts(0), Some((1, 0)));
			// Beginning of a new index.
			assert_eq!(Bet::index(), 1);
			assert_eq!(Bet::prices(), vec![]);
		});
	}

	#[test]
	fn bet_unbet_works() {
		new_test_ext().execute_with(|| {
			run_to_block(1);
			set_price(120);

			assert_ok!(Bet::bet(Some(1).into()));
			assert_ok!(Bet::unbet(Some(1).into()));
			assert_ok!(Bet::collect(Some(1).into()));
			assert!(account_is_liquid(&1));
			assert_eq!(Balances::free_balance(&1), 10);
		});
	}

	#[test]
	fn bet_locking_works() {
		new_test_ext().execute_with(|| {
			System::set_block_number(1);
			set_price(120);
			assert_ok!(Bet::bet(Some(1).into()));
			assert!(!account_is_liquid(&1));
		});
	}

	#[test]
	fn bet_invalid_collect_should_not_work() {
		new_test_ext().execute_with(|| {
			System::set_block_number(1);
			set_price(120);
			assert_ok!(Bet::bet(Some(1).into()));

			assert_eq!(Bet::incoming(), 10);
			assert!(!account_is_liquid(&1));
			assert_eq!(Balances::free_balance(&1), 10);

			run_to_next_index();

			assert_eq!(Bet::index(), 1);
			assert_eq!(Bet::total(), 10);

			assert_ok!(Bet::unbet(Some(1).into()));

			assert!(!account_is_liquid(&1));
			assert_ok!(Bet::collect(Some(1).into()));
			assert!(!account_is_liquid(&1));
		});
	}

	#[test]
	fn bet_win_unbet_collect_works() {
		new_test_ext().execute_with(|| {
			System::set_block_number(1);
			// index == 0
			set_price(120);
			assert_ok!(Bet::bet(Some(1).into()));

			assert_eq!(Bet::incoming(), 10);
			assert!(!account_is_liquid(&1));
			assert_eq!(Balances::free_balance(&1), 10);

			run_to_next_index();
			// index == 1

			assert_eq!(Bet::index(), 1);
			assert_eq!(Bet::total(), 10);

			assert_ok!(Bet::unbet(Some(1).into()));
			assert!(!account_is_liquid(&1));
			assert_eq!(Bet::outgoing(), 10);

			Bet::contribute(10);
			set_price(100);

			run_to_next_index();
			// index == 2

			assert_ok!(Bet::collect(Some(1).into()));
			assert_eq!(Balances::free_balance(&1), 20);
			assert!(!account_is_liquid(&1));

			run_to_next_index();
			// index == 3
			assert_ok!(Bet::collect(Some(1).into()));
			assert!(account_is_liquid(&1));
		});
	}

	#[test]
	fn bet_lose_unbet_works() {
		new_test_ext().execute_with(|| {
			System::set_block_number(1);
			// index == 0
			set_price(120);
			assert_ok!(Bet::bet(Some(1).into()));

			assert_eq!(Bet::incoming(), 10);
			assert!(!account_is_liquid(&1));
			assert_eq!(Balances::free_balance(&1), 10);

			run_to_next_index();
			// index == 1

			assert_eq!(Bet::index(), 1);
			assert_eq!(Bet::total(), 10);

			assert_ok!(Bet::unbet(Some(1).into()));
			assert!(!account_is_liquid(&1));
			assert_eq!(Bet::outgoing(), 10);

			Bet::contribute(10);
			set_price(140);

			run_to_next_index();
			// index == 2

			assert_ok!(Bet::collect(Some(1).into()));
			assert_eq!(Balances::free_balance(&1), 5);
			assert!(account_is_liquid(&1));
			assert_eq!(Bet::total(), 0);
		});
	}

	#[test]
	fn duplicate_bet_is_noop() {
		new_test_ext().execute_with(|| {
			System::set_block_number(1);
			// index == 0
			set_price(120);
			assert_ok!(Bet::bet(Some(1).into()));
			assert_ok!(Bet::bet(Some(1).into()));

			assert_eq!(Bet::incoming(), 10);
			assert_eq!(Bet::outgoing(), 0);
			assert!(!account_is_liquid(&1));
			assert_eq!(Balances::free_balance(&1), 10);

			run_to_next_index();
			// index == 1

			assert_ok!(Bet::bet(Some(1).into()));
			assert_eq!(Bet::index(), 1);
			assert_eq!(Bet::total(), 10);
			assert_eq!(Bet::outgoing(), 0);
			assert_eq!(Bet::incoming(), 0);
			assert!(!account_is_liquid(&1));
			assert_eq!(Balances::free_balance(&1), 10);

			Bet::contribute(10);
			set_price(100);

			run_to_next_index();
			// index == 2

			assert_ok!(Bet::collect(Some(1).into()));
			assert_eq!(Balances::free_balance(&1), 20);
			assert!(!account_is_liquid(&1));
		});
	}

	#[test]
	fn duplicate_unbet_is_noop() {
		new_test_ext().execute_with(|| {
			System::set_block_number(1);
			// index == 0
			set_price(120);
			assert_ok!(Bet::bet(Some(1).into()));

			assert_eq!(Bet::incoming(), 10);
			assert!(!account_is_liquid(&1));
			assert_eq!(Balances::free_balance(&1), 10);

			run_to_next_index();
			// index == 1

			assert_eq!(Bet::index(), 1);
			assert_eq!(Bet::total(), 10);

			assert_ok!(Bet::unbet(Some(1).into()));
			assert_ok!(Bet::unbet(Some(1).into()));
			assert!(!account_is_liquid(&1));
			assert_eq!(Bet::outgoing(), 10);
			assert_eq!(Bet::incoming(), 0);

			Bet::contribute(10);
			set_price(100);

			run_to_next_index();
			// index == 2

			assert_ok!(Bet::unbet(Some(1).into()));
			assert_eq!(Bet::outgoing(), 0);
			assert_eq!(Bet::incoming(), 0);
			assert_ok!(Bet::collect(Some(1).into()));
			assert_eq!(Balances::free_balance(&1), 20);
			assert!(!account_is_liquid(&1));

			run_to_next_index();
			// index == 3
			assert_ok!(Bet::unbet(Some(1).into()));
			assert_eq!(Bet::outgoing(), 0);
			assert_eq!(Bet::incoming(), 0);
			assert_ok!(Bet::collect(Some(1).into()));
			assert!(account_is_liquid(&1));
		});
	}

	#[test]
	fn accumulated_bet_works() {
		new_test_ext().execute_with(|| {
			System::set_block_number(1);
			// index == 0
			set_price(120);
			assert_ok!(Bet::bet(Some(1).into()));

			assert_eq!(Bet::incoming(), 10);
			assert_eq!(Bet::outgoing(), 0);
			assert!(!account_is_liquid(&1));
			assert_eq!(Balances::free_balance(&1), 10);

			run_to_next_index();
			// index == 1

			assert_eq!(Bet::index(), 1);
			assert_eq!(Bet::total(), 10);

			assert_eq!(Bet::incoming(), 0);
			assert_eq!(Bet::outgoing(), 0);
			assert!(!account_is_liquid(&1));
			assert_eq!(Balances::free_balance(&1), 10);

			Bet::contribute(10);
			set_price(100);

			run_to_next_index();
			// index == 2

			assert_eq!(Bet::index(), 2);
			assert_eq!(Bet::total(), 20);

			assert_ok!(Bet::unbet(Some(1).into()));

			assert_eq!(Bet::incoming(), 0);
			assert_eq!(Bet::outgoing(), 20);
			assert!(!account_is_liquid(&1));
			assert_eq!(Balances::free_balance(&1), 20);

			Bet::contribute(10);
			set_price(80);

			run_to_next_index();
			// index == 3

			assert_eq!(Bet::index(), 3);
			assert_eq!(Bet::total(), 0);

			assert_ok!(Bet::collect(Some(1).into()));

			assert_eq!(Bet::incoming(), 0);
			assert_eq!(Bet::outgoing(), 0);
			assert!(!account_is_liquid(&1));
			assert_eq!(Balances::free_balance(&1), 30);

			Bet::contribute(10);
			set_price(60);

			run_to_next_index();
			// index == 4

			assert_eq!(Bet::index(), 4);
			assert_eq!(Bet::total(), 0);

			assert_ok!(Bet::collect(Some(1).into()));

			assert_eq!(Bet::incoming(), 0);
			assert_eq!(Bet::outgoing(), 0);
			assert!(account_is_liquid(&1));
			assert_eq!(Balances::free_balance(&1), 30);
		});
	}

	#[test]
	fn unbet_bet_is_noop() {
		new_test_ext().execute_with(|| {
			System::set_block_number(1);
			// index == 0
			set_price(120);
			assert_ok!(Bet::bet(Some(1).into()));

			assert_eq!(Bet::incoming(), 10);
			assert_eq!(Bet::outgoing(), 0);
			assert!(!account_is_liquid(&1));
			assert_eq!(Balances::free_balance(&1), 10);

			run_to_next_index();
			// index == 1

			assert_eq!(Bet::index(), 1);
			assert_eq!(Bet::total(), 10);

			assert_ok!(Bet::unbet(Some(1).into()));

			assert_eq!(Bet::incoming(), 0);
			assert_eq!(Bet::outgoing(), 10);

			assert_ok!(Bet::bet(Some(1).into()));

			assert_eq!(Bet::incoming(), 0);
			assert_eq!(Bet::outgoing(), 0);
			assert!(!account_is_liquid(&1));
			assert_eq!(Balances::free_balance(&1), 10);

			Bet::contribute(10);
			set_price(100);

			run_to_next_index();
			// index == 2

			assert_eq!(Bet::index(), 2);
			assert_eq!(Bet::total(), 20);

			assert_ok!(Bet::unbet(Some(1).into()));

			assert_eq!(Bet::incoming(), 0);
			assert_eq!(Bet::outgoing(), 20);
			assert!(!account_is_liquid(&1));
			assert_eq!(Balances::free_balance(&1), 20);

			Bet::contribute(10);
			set_price(80);

			run_to_next_index();
			// index == 3

			assert_eq!(Bet::index(), 3);
			assert_eq!(Bet::total(), 0);

			assert_ok!(Bet::collect(Some(1).into()));

			assert_eq!(Bet::incoming(), 0);
			assert_eq!(Bet::outgoing(), 0);
			assert!(!account_is_liquid(&1));
			assert_eq!(Balances::free_balance(&1), 30);

			run_to_next_index();
			// index == 4

			assert_eq!(Bet::index(), 4);
			assert_eq!(Bet::total(), 0);

			assert_ok!(Bet::collect(Some(1).into()));

			assert_eq!(Bet::incoming(), 0);
			assert_eq!(Bet::outgoing(), 0);
			assert!(account_is_liquid(&1));
			assert_eq!(Balances::free_balance(&1), 30);
		});
	}
}
