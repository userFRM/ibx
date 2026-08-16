//! What the account holds and what it is worth.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::collections::HashMap;
use crate::types::*;

/// Account snapshot, per-position info, and atomic instrument positions.
pub struct PortfolioState {
    account: Mutex<AccountState>,
    /// Every figure the venue states about the account, under the name it
    /// states it by and in the currency it states it in.
    ///
    /// A named few were read into fields and the rest were dropped where they
    /// arrived, with nothing to say they had come. The venue states a great
    /// many more than any client names, and a figure nobody named is still a
    /// figure about the account.
    stated_account_values: Mutex<Vec<(String, String, String)>>,
    /// True once the first gateway account message ("UT"/"UM"/"RL") has been received.
    account_data_received: AtomicBool,
    /// True once the CCP init burst has been fully processed.
    account_download_complete: AtomicBool,
    /// Position info (conId -> PositionInfo) for reqPositions and P&L.
    position_infos: Mutex<HashMap<i64, PositionInfo>>,
    /// Holdings the venue reports that this broker does not itself hold:
    /// positions held away, and rows it marks as shown but not held. Kept
    /// apart from the account's own, which is what a caller asking for
    /// positions means.
    positions_elsewhere: Mutex<HashMap<i64, crate::types::PositionElsewhere>>,
    /// Account figures for the holdings the account does not hold itself,
    /// keyed by which set they describe and what they are called.
    values_elsewhere: Mutex<HashMap<(crate::types::HeldElsewhere, String), String>>,
    positions: [AtomicU64; MAX_INSTRUMENTS],
    /// Midnight seeds from 6040=143 for client-side daily P&L computation.
    midnight_seeds: Mutex<HashMap<i64, MidnightSeed>>,
    /// Correlation id the venue stamped on the seeds it last sent.
    pnl_request_key: Mutex<String>,
    /// Prices the venue states per contract (6040=152), kept as it wrote them
    /// so a price that does not read as a number cannot displace the rest.
    venue_prices: Mutex<HashMap<i64, String>>,
}

impl PortfolioState {
    pub(super) fn new() -> Self {
        Self {
            account: Mutex::new(AccountState::default()),
            stated_account_values: Mutex::new(Vec::new()),
            account_data_received: AtomicBool::new(false),
            account_download_complete: AtomicBool::new(false),
            position_infos: Mutex::new(HashMap::new()),
            positions_elsewhere: Mutex::new(HashMap::new()),
            values_elsewhere: Mutex::new(HashMap::new()),
            positions: std::array::from_fn(|_| AtomicU64::new(0)),
            midnight_seeds: Mutex::new(HashMap::new()),
            pnl_request_key: Mutex::new(String::new()),
            venue_prices: Mutex::new(HashMap::new()),
        }
    }

    /// Read account state snapshot.
    pub fn account(&self) -> AccountState {
        *self.account.lock().unwrap()
    }

    /// Get all position infos (for reqPositions).
    /// Holdings the venue reports that this broker does not hold itself.
    pub fn positions_elsewhere(&self) -> Vec<crate::types::PositionElsewhere> {
        self.positions_elsewhere.lock().unwrap().values().cloned().collect()
    }

    /// Record one, as the venue states it. The venue restates a row rather
    /// than withdrawing it, so this replaces what it holds for that contract.
    #[doc(hidden)] pub fn set_position_elsewhere(&self, row: crate::types::PositionElsewhere) {
        self.positions_elsewhere.lock().unwrap().insert(row.con_id, row);
    }

    /// The account figures describing one of the sets of holdings the account
    /// does not hold itself, as name and value.
    pub fn values_elsewhere(&self, held: crate::types::HeldElsewhere) -> Vec<(String, String)> {
        self.values_elsewhere.lock().unwrap()
            .iter()
            .filter(|((set, _), _)| *set == held)
            .map(|((_, name), value)| (name.clone(), value.clone()))
            .collect()
    }

    #[doc(hidden)] pub fn set_value_elsewhere(
        &self, held: crate::types::HeldElsewhere, name: String, value: String,
    ) {
        self.values_elsewhere.lock().unwrap().insert((held, name), value);
    }

    /// Every position held, as the caller reads one.
    pub fn position_infos(&self) -> Vec<PositionInfo> {
        self.position_infos.lock().unwrap().values().cloned().collect()
    }

    /// Get position info for a single conId (for pnlSingle).
    pub fn position_info(&self, con_id: i64) -> Option<PositionInfo> {
        self.position_infos.lock().unwrap().get(&con_id).cloned()
    }

    /// Read current position for an instrument.
    pub fn position(&self, id: InstrumentId) -> f64 {
        // Held as bits so the slot stays lock-free; a holding is fractional and
        // a whole-number one read half a share as flat.
        f64::from_bits(self.positions[id as usize].load(Ordering::Relaxed))
    }

    // ── Hot-loop-side writers ──

    /// True once at least one gateway account message has been processed.
    pub fn account_data_received(&self) -> bool {
        self.account_data_received.load(Ordering::Acquire)
    }

    /// Record a figure the venue stated, replacing any earlier statement of
    /// the same name in the same currency.
    #[doc(hidden)]
    pub fn note_account_value(&self, key: &str, value: &str, currency: &str) {
        // A figure the venue stated is account data having arrived. Marked
        // only when the typed copy was built, a summary asked for in between
        // was answered with nothing at all — and marked before the figure is
        // recorded, a summary asked for in between is answered with the same
        // nothing. The flag goes up once what it announces is readable.
        {
            let mut all = self.stated_account_values.lock().unwrap();
            match all.iter_mut().find(|(k, _, c)| k == key && c == currency) {
                Some(slot) => slot.1 = value.to_string(),
                None => all.push((key.to_string(), value.to_string(), currency.to_string())),
            }
        }
        self.account_data_received.store(true, Ordering::Release);
    }

    /// Every figure the venue has stated about the account, as it stated them.
    pub fn stated_account_values(&self) -> Vec<(String, String, String)> {
        self.stated_account_values.lock().unwrap().clone()
    }

    #[doc(hidden)] pub fn set_account(&self, account: &AccountState) {
        *self.account.lock().unwrap() = *account;
        self.account_data_received.store(true, Ordering::Release);
    }

    /// Mark account download as complete (init burst processed).
    #[doc(hidden)] pub fn set_account_download_complete(&self) {
        self.account_download_complete.store(true, Ordering::Release);
    }

    /// True once the CCP init burst has been fully processed.
    pub fn account_download_complete(&self) -> bool {
        self.account_download_complete.load(Ordering::Acquire)
    }

    #[doc(hidden)] pub fn set_position_info(&self, info: PositionInfo) {
        let mut map = self.position_infos.lock().unwrap();
        match map.get_mut(&info.con_id) {
            Some(existing) => {
                existing.position = info.position;
                // Written as given: a cost the row did not state is filled
                // in by the caller, which is the only side that can tell an
                // absent tag from a stated zero. A broker correcting a basis
                // to zero has to be able to say so.
                existing.avg_cost = info.avg_cost;
                if !info.symbol.is_empty() { existing.symbol = info.symbol; }
                if !info.sec_type.is_empty() { existing.sec_type = info.sec_type; }
                if !info.currency.is_empty() { existing.currency = info.currency; }
                if !info.multiplier.is_empty() { existing.multiplier = info.multiplier; }
                // Marks are owned by set_position_marks; leave them untouched so
                // the lean position feed can't zero them.
            }
            None => { map.insert(info.con_id, info); }
        }
    }

    /// Apply a fill of this account's own to the holding it changes.
    ///
    /// The broker states holdings on a feed of its own and does not restate
    /// them when an order fills, so a holding read back during the session is
    /// otherwise the one the session started with. The reference client keeps
    /// its own count between statements and so does this.
    ///
    /// `delta` is signed by side and `price` is what the fill paid, both per
    /// unit. Adding to a holding averages the new cost in; reducing one leaves
    /// the basis where it was, because a sale realises a gain and does not
    /// re-price what remains. A holding that closes carries no basis at all.
    #[doc(hidden)] pub fn apply_fill(&self, con_id: i64, delta: f64, price: Price) {
        if con_id == 0 || delta == 0.0 {
            return;
        }
        let mut map = self.position_infos.lock().unwrap();
        let row = map.entry(con_id).or_insert_with(|| PositionInfo { con_id, ..Default::default() });
        let before = row.position;
        let after = before + delta;
        row.position = after;
        if after == 0.0 {
            row.avg_cost = 0;
        } else if before == 0.0 || (before > 0.0) == (delta > 0.0) {
            let cost = row.avg_cost as f64 * before + price as f64 * delta;
            row.avg_cost = (cost / after) as Price;
        } else if (before > 0.0) != (after > 0.0) {
            // One fill that closed the holding and opened the opposite one.
            // Keeping the old basis prices a short against what a long paid,
            // and every profit and loss read afterwards is measured from the
            // wrong side. What is held now was bought at this price.
            row.avg_cost = price;
        }
    }

    /// Update the per-position marks (from the account-updates portfolio message).
    /// Kept separate from set_position_info so the lean position feed, which has
    /// no marks, does not overwrite them.
    #[doc(hidden)] pub fn set_position_marks(&self, con_id: i64, market_price: Price, market_value: Price, unrealized_pnl: Price, realized_pnl: Price) {
        let mut map = self.position_infos.lock().unwrap();
        let entry = map.entry(con_id).or_insert_with(|| PositionInfo { con_id, ..Default::default() });
        entry.market_price = market_price;
        entry.market_value = market_value;
        entry.unrealized_pnl = unrealized_pnl;
        entry.realized_pnl = realized_pnl;
    }

    #[doc(hidden)] pub fn set_position(&self, id: InstrumentId, pos: f64) {
        self.positions[id as usize].store(pos.to_bits(), Ordering::Relaxed);
    }

    /// Store midnight seeds from 6040=143 P&L response, under the correlation
    /// id the body was stamped with.
    #[doc(hidden)] pub fn set_midnight_seeds(&self, request_key: String, seeds: Vec<MidnightSeed>) {
        let mut map = self.midnight_seeds.lock().unwrap();
        map.clear();
        for s in seeds {
            map.insert(s.con_id, s);
        }
        *self.pnl_request_key.lock().unwrap() = request_key;
    }

    /// Read midnight seeds for client-side P&L computation.
    pub fn midnight_seeds(&self) -> Vec<MidnightSeed> {
        self.midnight_seeds.lock().unwrap().values().copied().collect()
    }

    /// The correlation id carried by the seeds now held.
    pub fn pnl_request_key(&self) -> String {
        self.pnl_request_key.lock().unwrap().clone()
    }

    /// Store the prices the venue states per contract (6040=152). A later table
    /// updates the contracts it names and leaves the others as they stood.
    #[doc(hidden)] pub fn set_venue_prices(&self, prices: HashMap<i64, String>) {
        self.venue_prices.lock().unwrap().extend(prices);
    }

    /// The price the venue states for a contract, as text.
    pub fn venue_price(&self, con_id: i64) -> Option<String> {
        self.venue_prices.lock().unwrap().get(&con_id).cloned()
    }
}
