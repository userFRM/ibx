//! The security-definition farm.
//!
//! A connection of its own, stated by the venue at logon beside the two this
//! client already opened. It carries the corporate-events calendar, which the
//! trading connection answers `Request not supported` for — the sub-protocol
//! is served here and nowhere else.
//!
//! Absent where the venue stated no route. That is a fact about the session,
//! not a failure: everything else works without it, and a session that refused
//! to open because one farm was down would be worse than one that says which
//! farms it has.

use std::sync::mpsc::SyncSender;
use std::time::Instant;

use crate::bridge::{Event, SharedState};
use crate::config::chrono_free_timestamp;
use crate::control::calendar as cal;
use crate::protocol::connection::{Connection, Frame};
use crate::protocol::fix;

use super::HeartbeatState;

/// How long a calendar request waits before it is given up on.
const CALENDAR_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

#[derive(Default)]
pub struct SecDefState {
    /// Calendar requests waiting on an answer: the name this client gave the
    /// request, which the answer echoes, which of the two it was, and when to
    /// stop waiting.
    pending: Vec<(String, u32, bool, Instant)>,
    /// Whether the event types have been asked for. The counterpart holds them
    /// and will not build an event request without them, so an event request
    /// sent first is one the venue is never asked in ordinary operation.
    meta_asked: bool,
    /// Message types this connection has sent that nothing here reads, named
    /// once each. Reported where somebody looks, rather than dropped.
    unread: std::collections::HashSet<String>,
}

impl SecDefState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Ask what event types the calendar carries.
    /// Stop waiting on a calendar query the caller no longer wants.
    ///
    /// The query is one message and one answer, so there is nothing at the
    /// venue to withdraw; what is withdrawn is the answer, which would
    /// otherwise be delivered to a caller who has said they are done with it.
    /// Answers whether there was one to withdraw, so a cancel naming nothing
    /// can say so rather than look like it acted.
    pub(crate) fn withdraw_calendar_request(&mut self, req_id: u32) -> bool {
        let before = self.pending.len();
        self.pending.retain(|(_, waiting, ..)| *waiting != req_id);
        self.pending.len() != before
    }

    pub(crate) fn send_calendar_meta_data_request(
        &mut self,
        req_id: u32,
        conn: &mut Option<Connection>,
        hb: &mut HeartbeatState,
        shared: &SharedState,
    ) {
        let Some(conn) = conn.as_mut() else {
            shared.reference.push_historical_error(
                req_id,
                321,
                "the calendar is carried on a connection this session does not have"
                    .to_string(),
            );
            return;
        };
        let key = format!("MetaDataRequest{req_id}");
        let ts = chrono_free_timestamp();
        let json = cal::meta_data_request();
        if let Err(e) = conn.send_fix(&[
            (fix::TAG_MSG_TYPE, "U"),
            (fix::TAG_SENDING_TIME, &ts),
            (6040, &cal::CALENDAR_SUB_PROTOCOL.to_string()),
            (cal::TAG_CALENDAR_KEY, &key),
            (cal::TAG_CALENDAR_REQUEST_KIND, &cal::CALENDAR_META_DATA.to_string()),
            (cal::TAG_CALENDAR_JSON, &json),
        ]) {
            shared.reference.push_historical_error(req_id, 504, format!("not sent: {e}"));
            return;
        }
        hb.last_secdef_sent = Instant::now();
        self.meta_asked = true;
        self.pending.push((key, req_id, true, Instant::now() + CALENDAR_TIMEOUT));
        log::info!("Sent calendar metadata request: req_id={req_id}");
    }

    /// Ask the calendar for events.
    pub(crate) fn send_calendar_events_request(
        &mut self,
        req_id: u32,
        query: &cal::CalendarQuery,
        conn: &mut Option<Connection>,
        hb: &mut HeartbeatState,
        shared: &SharedState,
    ) {
        if !self.meta_asked {
            shared.reference.push_historical_error(
                req_id,
                321,
                "the calendar's event types have not been asked for; request them first"
                    .to_string(),
            );
            return;
        }
        let json = match cal::event_data_request(query) {
            Ok(json) => json,
            Err(why) => {
                shared.reference.push_historical_error(req_id, 321, why);
                return;
            }
        };
        let Some(conn) = conn.as_mut() else {
            shared.reference.push_historical_error(
                req_id,
                321,
                "the calendar is carried on a connection this session does not have"
                    .to_string(),
            );
            return;
        };
        let key = format!("CalendarRequest{req_id}");
        let ts = chrono_free_timestamp();
        if let Err(e) = conn.send_fix(&[
            (fix::TAG_MSG_TYPE, "U"),
            (fix::TAG_SENDING_TIME, &ts),
            (6040, &cal::CALENDAR_SUB_PROTOCOL.to_string()),
            (cal::TAG_CALENDAR_KEY, &key),
            (cal::TAG_CALENDAR_REQUEST_KIND, &cal::CALENDAR_EVENT_DATA.to_string()),
            (cal::TAG_CALENDAR_JSON, &json),
        ]) {
            shared.reference.push_historical_error(req_id, 504, format!("not sent: {e}"));
            return;
        }
        hb.last_secdef_sent = Instant::now();
        self.pending.push((key, req_id, false, Instant::now() + CALENDAR_TIMEOUT));
        log::info!("Sent calendar events request: req_id={req_id}");
    }

    /// Read whatever this connection has sent.
    pub(crate) fn poll(
        &mut self,
        conn: &mut Option<Connection>,
        shared: &SharedState,
        event_tx: &Option<SyncSender<Event>>,
        hb: &mut HeartbeatState,
    ) {
        if let Err(lost) = self.read(conn, shared, event_tx, hb) {
            // A connection that has gone is put down rather than kept and
            // written to. Kept, every later request was sent into a socket
            // that would never answer and waited out its own timeout; put
            // down, a caller is told at once that this session has no
            // connection for the calendar.
            *conn = None;
            for (_, req_id, ..) in self.pending.drain(..) {
                shared.reference.push_historical_error(
                    req_id,
                    504,
                    format!("the connection carrying the calendar went: {lost}"),
                );
            }
            self.meta_asked = false;
        }
    }

    fn read(
        &mut self,
        conn: &mut Option<Connection>,
        shared: &SharedState,
        event_tx: &Option<SyncSender<Event>>,
        hb: &mut HeartbeatState,
    ) -> Result<(), String> {
        self.sweep(shared);
        let messages = match conn.as_mut() {
            None => return Ok(()),
            Some(conn) => {
                match conn.try_recv() {
                    Ok(0) if !conn.has_buffered_data() => return Ok(()),
                    Ok(0) => {}
                    Err(e) => {
                        log::error!("Security definition farm connection lost: {e}");
                        return Err(e.to_string());
                    }
                    Ok(_) => hb.last_secdef_recv = Instant::now(),
                }
                let frames = conn.extract_frames();
                let mut msgs: Vec<Vec<u8>> = Vec::new();
                for frame in &frames {
                    match frame {
                        Frame::FixComp(raw) => {
                            let Some(unsigned) = conn.unsign(raw) else { continue };
                            match crate::protocol::fixcomp::fixcomp_decompress(&unsigned) {
                                Ok(inner) => msgs.extend(inner),
                                Err(e) => log::warn!(
                                    "Security definition farm: dropping a malformed frame: {e}",
                                ),
                            }
                        }
                        Frame::Fix(raw) | Frame::Binary(raw) => {
                            let Some(unsigned) = conn.unsign(raw) else { continue };
                            msgs.push(unsigned);
                        }
                        Frame::Control(_) => {}
                    }
                }
                msgs
            }
        };
        for msg in &messages {
            self.handle(msg, conn, shared, event_tx, hb);
        }
        Ok(())
    }

    /// Give up on a request the venue never answered.
    ///
    /// A caller waiting on an answer that is not coming cannot tell that apart
    /// from a slow venue, so the wait is bounded and the caller told.
    fn sweep(&mut self, shared: &SharedState) {
        let now = Instant::now();
        let mut expired = Vec::new();
        self.pending.retain(|(_, req_id, _, deadline)| {
            if *deadline > now {
                true
            } else {
                expired.push(*req_id);
                false
            }
        });
        for req_id in expired {
            shared.reference.push_historical_error(
                req_id,
                321,
                "the calendar did not answer".to_string(),
            );
        }
    }

    fn handle(
        &mut self,
        msg: &[u8],
        conn: &mut Option<Connection>,
        shared: &SharedState,
        event_tx: &Option<SyncSender<Event>>,
        hb: &mut HeartbeatState,
    ) {
        let parsed = fix::fix_parse(msg);
        let Some(msg_type) = parsed.get(&fix::TAG_MSG_TYPE) else { return };
        match msg_type.as_str() {
            "0" => {}
            // The venue asking whether this connection is still there. Left
            // unanswered it closes the connection, and the calendar with it.
            "1" => {
                let test_id = parsed.get(&fix::TAG_TEST_REQ_ID).cloned().unwrap_or_default();
                if let Some(conn) = conn.as_mut() {
                    let ts = chrono_free_timestamp();
                    let _ = conn.send_fix(&[
                        (fix::TAG_MSG_TYPE, fix::MSG_HEARTBEAT),
                        (fix::TAG_SENDING_TIME, &ts),
                        (fix::TAG_TEST_REQ_ID, &test_id),
                    ]);
                    hb.last_secdef_sent = Instant::now();
                }
            }
            "U" => match parsed.get(&6040).map(String::as_str) {
                Some(cal::CALENDAR_ANSWER) => self.deliver(&parsed, false, shared, event_tx),
                Some(cal::CALENDAR_REFUSAL) => self.deliver(&parsed, true, shared, event_tx),
                Some(other) if self.unread.insert(other.to_string()) => {
                    log::info!("Unread on the security definition farm: sub-protocol {other}");
                }
                Some(_) => {}
                None => {}
            },
            // A rejection carries no request id, so it belongs to whatever is
            // outstanding. With one request in flight that is unambiguous.
            "3" => {
                let said = parsed.get(&58).cloned().unwrap_or_else(|| "refused".to_string());
                // A reject carries no request id, so it belongs to whatever is
                // outstanding. Answering only when exactly one thing was
                // waiting left every other caller to wait out a timeout for a
                // refusal the venue had already given — and the calendar is
                // asked in twos, the event types and then the events.
                if self.pending.is_empty() {
                    log::warn!("Security definition farm refused something: {said}");
                    return;
                }
                for (_, req_id, ..) in self.pending.drain(..) {
                    shared.reference.push_historical_error(req_id, 321, said.clone());
                }
                self.meta_asked = false;
            }
            other => {
                if self.unread.insert(other.to_string()) {
                    log::info!("Unread on the security definition farm: type {other}");
                }
            }
        }
    }

    /// The calendar's answer, or its refusal, to the caller that asked.
    fn deliver(
        &mut self,
        parsed: &std::collections::HashMap<u32, String>,
        refused: bool,
        shared: &SharedState,
        event_tx: &Option<SyncSender<Event>>,
    ) {
        let Some(key) = parsed.get(&cal::TAG_CALENDAR_KEY) else {
            log::warn!("A calendar answer arrived naming no request; dropping it");
            return;
        };
        let Some(at) = self.pending.iter().position(|(k, ..)| k == key) else {
            log::warn!("A calendar answer named '{key}', which nothing here asked for");
            return;
        };
        let (_, req_id, is_meta, _) = self.pending.remove(at);
        if refused {
            let said = parsed.get(&58).cloned().unwrap_or_else(|| "refused".to_string());
            shared.reference.push_historical_error(req_id, 321, said);
            return;
        }
        let json = parsed.get(&96).cloned().unwrap_or_default();
        if is_meta {
            shared.reference.push_calendar_meta_data(req_id, json.clone());
        } else {
            shared.reference.push_calendar_events(req_id, json.clone());
        }
        let _ = event_tx;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sent(of: impl FnOnce(&mut SecDefState, &mut Option<Connection>, &mut HeartbeatState, &SharedState)) -> String {
        use std::io::Read;
        let (conn, mut peer) = Connection::for_test();
        let mut conn = Some(conn);
        let mut hb = HeartbeatState::new();
        let shared = SharedState::new();
        let mut state = SecDefState::new();
        of(&mut state, &mut conn, &mut hb, &shared);
        let mut buf = [0u8; 8192];
        let n = peer.read(&mut buf).unwrap_or(0);
        String::from_utf8_lossy(&buf[..n]).replace('\u{1}', "|")
    }

    /// The calendar request goes out on this connection, whole.
    #[test]
    fn the_metadata_request_goes_out_whole() {
        let msg = sent(|state, conn, hb, shared| {
            state.send_calendar_meta_data_request(7, conn, hb, shared)
        });
        assert!(msg.contains("35=U|"), "{msg}");
        assert!(msg.contains("|6040=155|"), "the sub-protocol: {msg}");
        assert!(msg.contains("|8081=100|"), "which of the two: {msg}");
        assert!(msg.contains("|6556=MetaDataRequest7|"), "its own name: {msg}");
    }

    /// A session the venue stated no route for has no such connection, and a
    /// caller is told that rather than left waiting.
    #[test]
    fn without_the_connection_a_caller_is_told() {
        let shared = SharedState::new();
        let mut state = SecDefState::new();
        state.send_calendar_meta_data_request(7, &mut None, &mut HeartbeatState::new(), &shared);
        let told = shared.reference.drain_historical_errors_for_dispatch();
        assert_eq!(told.len(), 1);
        assert!(told[0].2.contains("does not have"), "{:?}", told[0]);
    }

    /// Events cannot be asked for before the event types are.
    #[test]
    fn events_before_metadata_are_refused_here() {
        let shared = SharedState::new();
        let mut state = SecDefState::new();
        let query = cal::CalendarQuery { con_id: Some(265598), ..Default::default() };
        state.send_calendar_events_request(9, &query, &mut None, &mut HeartbeatState::new(), &shared);
        let told = shared.reference.drain_historical_errors_for_dispatch();
        assert_eq!(told.len(), 1);
        assert!(told[0].2.contains("event types"), "{:?}", told[0]);
    }

    /// The answer names the request this client gave it, which is the only
    /// thing that says which caller is waiting.
    #[test]
    fn an_answer_reaches_the_caller_that_asked() {
        let shared = SharedState::new();
        let mut state = SecDefState::new();
        let mut conn = Some(Connection::for_test().0);
        state.send_calendar_meta_data_request(7, &mut conn, &mut HeartbeatState::new(), &shared);

        let mut reply = std::collections::HashMap::new();
        reply.insert(cal::TAG_CALENDAR_KEY, "MetaDataRequest7".to_string());
        reply.insert(96, r#"{"meta_data":{"event_types":[]}}"#.to_string());
        state.deliver(&reply, false, &shared, &None);

        let answered = shared.reference.drain_calendar_meta_data_for_dispatch();
        assert_eq!(answered.len(), 1);
        assert_eq!(answered[0].0, 7);
    }

    /// A refusal reaches every caller waiting, not just one. The calendar is
    /// asked in twos — the event types, then the events — so answering only
    /// when exactly one thing was outstanding left the other to wait out a
    /// timeout for a refusal already given.
    #[test]
    fn a_refusal_reaches_everyone_waiting() {
        let shared = SharedState::new();
        let mut state = SecDefState::new();
        let (socket, _peer) = Connection::for_test();
        let mut conn = Some(socket);
        state.send_calendar_meta_data_request(7, &mut conn, &mut HeartbeatState::new(), &shared);
        let query = cal::CalendarQuery { con_id: Some(1), ..Default::default() };
        state.send_calendar_events_request(8, &query, &mut conn, &mut HeartbeatState::new(), &shared);
        assert_eq!(
            state.pending.len(), 2,
            "both requests are outstanding; errors so far: {:?}",
            shared.reference.drain_historical_errors_for_dispatch(),
        );

        let reject = b"35=3\x0158=Request not supported  #155\x01";
        state.handle(reject, &mut conn, &shared, &None, &mut HeartbeatState::new());

        let told = shared.reference.drain_historical_errors_for_dispatch();
        assert_eq!(told.len(), 2, "somebody was left waiting");
        assert!(state.pending.is_empty());
    }

    /// A connection that has gone is put down rather than kept and written
    /// to. Kept, every later request went into a socket that would never
    /// answer and waited out its own timeout.
    #[test]
    fn a_connection_that_went_is_put_down() {
        let shared = SharedState::new();
        let mut state = SecDefState::new();
        let (conn, peer) = Connection::for_test();
        let mut conn = Some(conn);
        state.send_calendar_meta_data_request(7, &mut conn, &mut HeartbeatState::new(), &shared);
        drop(peer);

        // Read until the dead socket is noticed.
        for _ in 0..4 {
            state.poll(&mut conn, &shared, &None, &mut HeartbeatState::new());
        }
        if conn.is_none() {
            let told = shared.reference.drain_historical_errors_for_dispatch();
            assert!(!told.is_empty(), "the caller was left waiting on a dead socket");
        }
    }

    /// A request the venue never answers is given up on, and the caller told.
    /// Waiting forever is indistinguishable from a slow venue.
    #[test]
    fn a_request_that_is_never_answered_is_given_up_on() {
        let shared = SharedState::new();
        let mut state = SecDefState::new();
        state.pending.push((
            "MetaDataRequest7".to_string(),
            7,
            true,
            Instant::now() - std::time::Duration::from_secs(1),
        ));
        state.sweep(&shared);
        let told = shared.reference.drain_historical_errors_for_dispatch();
        assert_eq!(told.len(), 1, "the caller was left waiting");
        assert!(state.pending.is_empty());
    }
}
