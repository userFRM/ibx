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
    /// Holdings a download in progress has not restated yet.
    ///
    /// The venue states every holding the account has while the request is
    /// open, so one it does not state is one the account no longer has. Filled
    /// the moment a download begins and emptied as the rows arrive, whatever
    /// is still here when the download ends went unstated.
    awaiting_restatement: Mutex<std::collections::HashSet<i64>>,
    /// The account request whose end means every holding has been stated.
    ///
    /// A session asks the venue to restate the account more than once — a
    /// caller can ask for a refresh at any moment — and each request ends with
    /// a record of its own. Reconciled against whichever ended first, a
    /// refresh issued before the reconnect's rows arrived would have read as
    /// "the venue named nothing" and closed every holding the account has.
    restating_under: Mutex<Option<String>>,
    /// Holdings that have moved since a caller last read them.
    ///
    /// `reqPositions` is a real-time subscription, so a caller is told each
    /// time a holding changes and not only once when it asks. Held by
    /// contract id rather than by value: a holding that moves twice before
    /// the caller reads is delivered once, stating what it holds now.
    position_changes: Mutex<std::collections::BTreeSet<i64>>,
    /// Holdings the venue reports that this broker does not itself hold:
    /// positions held away, and rows it marks as shown but not held. Kept
    /// apart from the account's own, which is what a caller asking for
    /// positions means.
    positions_elsewhere: Mutex<HashMap<i64, crate::types::PositionElsewhere>>,
    /// Account figures for the holdings the account does not hold itself,
    /// keyed by which set they describe and what they are called.
    values_elsewhere: Mutex<HashMap<(crate::types::HeldElsewhere, String), String>>,
    positions: Box<[AtomicU64]>,
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
            awaiting_restatement: Mutex::new(std::collections::HashSet::new()),
            restating_under: Mutex::new(None),
            position_changes: Mutex::new(std::collections::BTreeSet::new()),
            positions_elsewhere: Mutex::new(HashMap::new()),
            values_elsewhere: Mutex::new(HashMap::new()),
            positions: (0..MAX_INSTRUMENTS).map(|_| AtomicU64::new(0)).collect(),
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
    ///
    /// Returns the holdings the download never restated. The venue states
    /// every holding the account has while the request is open, so one it did
    /// not name is one the account no longer has — left standing it goes on
    /// being reported as held and every exposure decision reads it that way.
    /// Their rows are set to nothing here; the instrument slots beside them
    /// belong to the caller, which is the side that can name an instrument.
    #[doc(hidden)] pub fn set_account_download_complete(&self, ends: &str) -> Vec<i64> {
        // Only the request that was asked to restate everything since the
        // download began. Another request's end says nothing about what this
        // one has stated so far.
        let mut under = self.restating_under.lock().unwrap();
        if under.as_deref() != Some(ends) {
            // A request still outstanding is what ends the download. Where
            // none is, this is the opening sequence's own end and there is
            // nothing to square — but where one is, this end belongs to some
            // other request and says nothing about what that one has stated.
            if under.is_none() {
                self.account_download_complete.store(true, Ordering::Release);
            }
            return Vec::new();
        }
        *under = None;
        drop(under);
        let unstated = std::mem::take(&mut *self.awaiting_restatement.lock().unwrap());
        let mut held = self.position_infos.lock().unwrap();
        let mut moved = self.position_changes.lock().unwrap();
        unstated.into_iter().filter(|con_id| match held.get_mut(con_id) {
            Some(info) if info.position != 0.0 => {
                info.position = 0.0;
                // And what the holding was worth, which is nothing. The basis
                // in particular: a row closing a holding takes its basis with
                // it, or the next position opened in the same contract is
                // given the dead one's when its own row states none. The marks
                // go for the same reason — a figure against a holding of
                // nothing is not a figure.
                info.avg_cost = 0;
                info.market_price = 0;
                info.market_value = 0;
                info.unrealized_pnl = 0;
                info.unrealized_stated = false;
                // Recorded as a move like any other, so a caller watching
                // holdings is told this one went to nothing rather than
                // reading the last figure it was given for ever.
                moved.insert(*con_id);
                true
            }
            _ => false,
        }).collect()
    }

    /// The download is over, and every holding it left unstated has been
    /// squared with it.
    ///
    /// Said last, and by the caller, because the caller owns the rest of the
    /// squaring — the instrument slots and the engine's own book. Said first,
    /// a reader waiting on it was let through while the holdings the download
    /// never named were still standing, which is the answer this exists to
    /// prevent.
    #[doc(hidden)] pub fn account_download_is_settled(&self) {
        self.account_download_complete.store(true, Ordering::Release);
    }

    /// True once the CCP init burst has been fully processed.
    pub fn account_download_complete(&self) -> bool {
        self.account_download_complete.load(Ordering::Acquire)
    }

    /// A new connection has not yet stated what the account holds.
    ///
    /// The flag above belongs to the connection that earned it. Left set
    /// across a reconnect it still reports the previous connection's download
    /// as finished, and a caller asking what the account holds is answered at
    /// once from the pre-drop snapshot while the venue's own statement is
    /// still arriving — so a holding that moved or closed while the
    /// connection was down is handed back as though it still stood.
    #[doc(hidden)] pub fn account_download_is_pending(&self) {
        self.account_download_complete.store(false, Ordering::Release);
        // And whether anything has been heard at all. Stored once at the first
        // account message of the session and cleared nowhere, every wait that
        // reads it stopped meaning anything after that.
        self.account_data_received.store(false, Ordering::Release);
        *self.awaiting_restatement.lock().unwrap() =
            self.position_infos.lock().unwrap().keys().copied().collect();
        // Nothing has been asked to restate them yet.
        *self.restating_under.lock().unwrap() = None;
    }

    /// The account request now outstanding, which will restate every holding.
    ///
    /// The first one asked since the download began, and only that one. A
    /// caller can ask for a refresh at any moment, and a later request taking
    /// this over left the earlier request's unstated holdings to be settled by
    /// the later one's end — so holdings the account still had, which the
    /// first request had simply not reached yet, were closed on the strength
    /// of a request that was never asked to state them.
    #[doc(hidden)] pub fn holdings_restated_under(&self, key: &str) {
        let mut under = self.restating_under.lock().unwrap();
        if under.is_none() {
            *under = Some(key.to_string());
        }
    }

    /// The download in progress has named this contract.
    ///
    /// Naming it is what counts, not writing a row for it: an entry that names
    /// a holding and carries no quantity states no quantity, and the venue
    /// sends those — so read off the row write, a holding the venue had just
    /// named was closed for never having been mentioned.
    #[doc(hidden)] pub fn note_restated(&self, con_id: i64) {
        self.awaiting_restatement.lock().unwrap().remove(&con_id);
    }

    #[doc(hidden)] pub fn set_position_info(&self, info: PositionInfo) {
        let con_id = info.con_id;
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
        // Recorded while the holdings are still held, so the two locks are
        // always taken in the same order and a reader cannot see the new
        // value before the record that it moved. Taken the other way round, a
        // drain running alongside this read the moved holding and then found
        // it again on the next drain, reporting one move twice.
        self.position_changes.lock().unwrap().insert(con_id);
    }

    /// The holdings that have moved since this was last called, as they stand
    /// now. Empty where nothing has moved.
    pub fn drain_position_changes(&self) -> Vec<PositionInfo> {
        let map = self.position_infos.lock().unwrap();
        let changed = std::mem::take(&mut *self.position_changes.lock().unwrap());
        changed.iter().filter_map(|c| map.get(c).cloned()).collect()
    }

    /// Update the per-position marks (from the account-updates portfolio message).
    /// Kept separate from set_position_info so the lean position feed, which has
    /// no marks, does not overwrite them.
    /// Apply the marks a frame states, and only those.
    ///
    /// A frame that states none of a figure is not a frame stating nought for
    /// it: written that way an absent tag overwrites a real profit with zero
    /// and the caller reads a holding as flat. The same rule the average cost
    /// beside them follows.
    #[doc(hidden)] pub fn set_position_marks(&self, con_id: i64, market_price: Option<Price>, market_value: Option<Price>, unrealized_pnl: Option<Price>, realized_pnl: Option<Price>) {
        let mut map = self.position_infos.lock().unwrap();
        let entry = map.entry(con_id).or_insert_with(|| PositionInfo { con_id, ..Default::default() });
        if let Some(v) = market_price { entry.market_price = v; }
        if let Some(v) = market_value { entry.market_value = v; }
        if let Some(v) = unrealized_pnl { entry.unrealized_pnl = v; entry.unrealized_stated = true; }
        if let Some(v) = realized_pnl { entry.realized_pnl = v; }
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
