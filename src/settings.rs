//! The settings a gateway keeps in its own configuration file.
//!
//! A gateway is a process, and a process is configured by a file beside it and
//! a window in front of it. This client is a library and has neither, so the
//! same settings belong on the client, where a caller sets them in code and
//! reads them back.
//!
//! They are stated on [`EClientConfig`](crate::api::client::EClientConfig)
//! alongside the login, because a caller has one session and should not have
//! to configure it in two places.
//!
//! **Each session runs under its own.** They are settled once, as the session
//! opens, into a [`SessionSettings`] the session carries: two sessions in one
//! process have their own, and neither can change the other's. What a caller
//! states wins over the environment, which wins over the default, so a program
//! configured the old way keeps working.
//!
//! Logging is the exception, and is named as one: a process has one logger, and
//! whoever installs it holds what flushes it, so `log_level`, `log_dir` and
//! `log_queue` belong to the process rather than to a session. The first
//! session to open installs the logger from them, the same way a gateway reads
//! its logging configuration as it starts; a session that opens after that
//! cannot move a logger that is already running, and one that states them then
//! is told so rather than left believing it was heard. See
//! [`logging::apply`](crate::logging::apply).

/// A setting stated on the client, or left to the environment.
///
/// Every one of these stands in for something the gateway held. Where a caller
/// states nothing, what was already in the environment stands — so a program
/// configured the old way keeps working, and one configured in code does not
/// have to know the environment exists.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GatewaySettings {
    /// The time zone the gateway ran in, which it announced at logon. This
    /// announces the same one and no more: a time handed to this client
    /// without a zone of its own is read as UTC whatever this says. Defaults
    /// to UTC.
    pub timezone: Option<String>,
    /// The locale it announced itself with. Reaches the wire through
    /// [`encoded`](Self::encoded), whose locale segment it replaces, so
    /// stating both leaves this one with nothing to do.
    pub locale: Option<String>,
    /// The build it announced itself as, which the venue keeps a list of and
    /// stops accepting when it is old enough.
    pub build: Option<String>,
    /// The version beside that build.
    pub version: Option<String>,
    /// The longer string it announced with them. A session resumed from an
    /// earlier one announces that one's instead, because it is the identity
    /// the server holds the session under.
    pub encoded: Option<String>,
    /// The machine identity it presented. Resumed the same way
    /// [`encoded`](Self::encoded) is.
    pub hardware_id: Option<String>,
    /// The host every farm connection is opened on, where it is not the one
    /// the venue names.
    pub market_data_host: Option<String>,
    /// The port a farm connection opens on, where the venue's routing names
    /// none. Logging in is always on the port the protocol fixes for it.
    pub port: Option<u16>,
    /// How long it waited to be admitted, in milliseconds.
    pub registration_timeout_ms: Option<u64>,
    /// How much it wrote down. Logging reads this from `IBX_LOG_LEVEL`.
    pub log_level: Option<String>,
    /// Where it wrote it. Logging reads this from `IBX_LOG_DIR`.
    pub log_dir: Option<String>,
    /// How many records it buffered before dropping them. Logging reads this
    /// from `IBX_LOG_QUEUE`, and reads it as a count: a boolean here could
    /// state nothing the reader understood, so every value fell to the
    /// default.
    pub log_queue: Option<usize>,

    // ── What the gateway did with what it received ──
    /// Which executions arrive when a session opens: today's, or every one
    /// the venue still holds. The gateway asks for every one.
    pub execution_reports: Option<ExecutionReportScope>,
    /// Whether a US stock trading on Nasdaq is handed back under the older
    /// spelling. The gateway does, so a program written against it compares
    /// against that spelling.
    pub island_for_nasdaq: Option<bool>,
}

/// Which executions a session asks for when it opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionReportScope {
    /// Today's only.
    Today,
    /// Every one the venue still holds, which is what the gateway asks for.
    All,
}

/// Gateway settings that are not settings here, and what to do instead.
///
/// Named rather than dropped: someone moving off a gateway will look for them,
/// and "there is no such thing here" is an answer where silence is not. Some
/// of these do have a stand-in — it is simply not one of the settings above —
/// and the reason names it.
pub const UNAVAILABLE: &[(&str, &str)] = &[
    ("ApiMsgsPerSlice", "nothing paces what a caller sends; the pacing here is the subscription burst a reconnect replays, stated on ReconnectConfig"),
    ("ApiTimeSliceMillis", "nothing paces what a caller sends; the pacing here is the subscription burst a reconnect replays, stated on ReconnectConfig"),
    ("TimestampZone", "no setting chooses the zone a timestamp is stated in; a bar is stated on the zone the venue names beside it, or as seconds since the epoch where the request asked for that"),
    ("LocalServerPort", "no local socket to listen on; this client is the client"),
    ("LocalApiPort", "no local socket to listen on; this client is the client"),
    ("TrustedIPs", "nothing connects to this client, so nothing needs trusting"),
    ("Local_FIX_Server_Settings", "no local socket to listen on; this client is the client, and the only sockets it holds are the ones it opened to the venue"),
    ("ApiOnly", "stated per session rather than once for a process: `readonly` on the client config, or connect(readonly=True)"),
    ("RemoteHostOrderRouting", "orders are routed on the connection the login opened, so this is `host` on the client config, or connect(host=...); left unset, the venue names the server this account is on"),
    ("RemotePortOrderRouting", "one port, fixed by the protocol: a redirect naming another is accepted at the socket and then reset, and the session only completes on the fixed one"),
    ("useSsl", "no switch: the login and the order connection are TLS with no plaintext path, and a farm connection is opened the one way the venue answers — a key exchange, an enciphered logon, then messages signed rather than enciphered"),
    ("UseSSL", "no switch: the login and the order connection are TLS with no plaintext path, and a farm connection is opened the one way the venue answers — a key exchange, an enciphered logon, then messages signed rather than enciphered"),
    ("reconnectOnSocketErr", "recovery is on unless a session turns it off, which is `reconnect` on the Rust client config, where ReconnectPolicy::Manual reports the loss and waits instead; a Python session always recovers"),
    ("Select_account_type", "nothing here selects one: the login decides, the venue names the accounts it holds at logon, and whether a session is a paper one is stated as `paper` on the client config, or connect(paper=True)"),
    ("MainWindow.Width", "no window"),
    ("MainWindow.Height", "no window"),
    ("vmoptions", "no runtime to size"),
];

/// Every setting a session runs under, resolved to a value.
///
/// Settled once, as the session opens, and immutable afterwards. Two sessions
/// in one process each hold their own, so one session's settings cannot reach
/// another session's reconnects through the process environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSettings {
    /// The zone the session announces at logon.
    pub timezone: String,
    /// The locale it announces itself for.
    pub locale: String,
    /// The build this session announces itself as.
    pub build: String,
    /// The version it announces.
    pub version: String,
    /// What the session encodes its client string as.
    pub encoded: String,
    /// What it identifies this machine as. Derived when unset.
    pub hardware_id: Option<String>,
    /// Which host every farm connection opens on, where the caller names one.
    pub market_data_host: Option<String>,
    /// Which port a farm connection opens on, where the venue's routing names
    /// none.
    pub port: u16,
    /// How long a call waits for the engine to name a contract before giving up.
    pub registration_timeout: std::time::Duration,
    /// Which executions a session asks for when it opens.
    pub execution_reports: ExecutionReportScope,
    /// Whether a US stock on Nasdaq is named by the older spelling. Takes the
    /// venue's grant as well as this setting.
    pub island_for_nasdaq: bool,
}

impl Default for SessionSettings {
    fn default() -> Self {
        GatewaySettings::default().resolve()
    }
}

impl GatewaySettings {
    /// Settle every setting: what the caller stated, else what the environment
    /// holds, else the default.
    ///
    /// The one place a session's settings read the environment. Called on the
    /// caller's thread as the session opens, before any thread of the engine's
    /// exists, so nothing downstream reads a setting that can still change.
    pub fn resolve(&self) -> SessionSettings {
        fn stated(caller: Option<&String>, variable: &str) -> Option<String> {
            caller
                .filter(|v| !v.is_empty())
                .cloned()
                .or_else(|| std::env::var(variable).ok().filter(|v| !v.is_empty()))
        }
        SessionSettings {
            timezone: stated(self.timezone.as_ref(), "IBX_TZ")
                .unwrap_or_else(|| "UTC".to_string()),
            locale: stated(self.locale.as_ref(), "IBX_LOCALE")
                .unwrap_or_else(|| crate::config::IB_LOCALE.to_string()),
            build: stated(self.build.as_ref(), "IBX_BUILD")
                .unwrap_or_else(|| crate::config::IB_BUILD.to_string()),
            version: stated(self.version.as_ref(), "IBX_VERSION")
                .unwrap_or_else(|| crate::config::IB_VERSION.to_string()),
            // The whole string, or the locale set into it, or neither. Tag
            // 6266 carries `{jdkVer}/{platform}/{locale}/{dist}` and the venue
            // refuses a locale that is not a canonical one.
            encoded: stated(self.encoded.as_ref(), "IBX_ENCODED").unwrap_or_else(|| {
                match stated(self.locale.as_ref(), "IBX_LOCALE") {
                    // The identity this client announces, with the locale
                    // segment replaced. Composing it a second time here makes a
                    // session that states a locale announce a stale runtime and
                    // platform while one that states none announces the current
                    // pair.
                    Some(locale) => match crate::config::IB_ENCODED.split('/')
                        .collect::<Vec<_>>()
                        .as_slice()
                    {
                        [runtime, platform, _, distribution] => {
                            format!("{runtime}/{platform}/{locale}/{distribution}")
                        }
                        _ => crate::config::IB_ENCODED.to_string(),
                    },
                    None => crate::config::IB_ENCODED.to_string(),
                }
            }),
            hardware_id: stated(self.hardware_id.as_ref(), "IBX_HWID"),
            market_data_host: stated(self.market_data_host.as_ref(), "IBX_FARM_HOST"),
            port: self
                .port
                .or_else(|| std::env::var("IBX_MISC_PORT").ok().and_then(|v| v.parse().ok()))
                .unwrap_or(crate::config::MISC_PORT),
            registration_timeout: self
                .registration_timeout_ms
                .or_else(|| {
                    std::env::var("IBX_REGISTRATION_TIMEOUT_MS").ok().and_then(|v| v.parse().ok())
                })
                .map_or(std::time::Duration::from_secs(5), std::time::Duration::from_millis),
            execution_reports: self.execution_reports.unwrap_or_else(|| {
                // However it is spelled, and said out loud when it is spelled
                // as neither. Matched against lowercase alone, `Today` fell to
                // the default and the session asked the venue for every
                // execution it still holds, which is the opposite of what was
                // stated and a heavier request on every session that opens.
                match std::env::var("IBX_EXECUTION_REPORTS") {
                    Ok(stated) if stated.eq_ignore_ascii_case("today") => {
                        ExecutionReportScope::Today
                    }
                    Ok(stated)
                        if !stated.is_empty() && !stated.eq_ignore_ascii_case("all") =>
                    {
                        log::warn!(
                            "IBX_EXECUTION_REPORTS names neither today nor all: {stated}. \
                             This session asks for every execution the venue holds",
                        );
                        ExecutionReportScope::All
                    }
                    _ => ExecutionReportScope::All,
                }
            }),
            island_for_nasdaq: self.island_for_nasdaq.unwrap_or_else(|| {
                // As above: `False` turned the setting on, because only the
                // lowercase spelling counted as off.
                !std::env::var("IBX_ISLAND_FOR_NASDAQ").is_ok_and(|stated| {
                    ["0", "false", "no"].iter().any(|off| stated.eq_ignore_ascii_case(off))
                })
            }),
        }
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    /// A setting stated on the client is what the session runs under.
    #[test]
    fn a_stated_setting_is_what_the_session_runs_under() {
        let resolved = GatewaySettings {
            timezone: Some("America/New_York".to_string()),
            port: Some(4002),
            ..Default::default()
        }
        .resolve();
        assert_eq!(resolved.timezone, "America/New_York");
        assert_eq!(resolved.port, 4002);
    }

    /// Two sessions in one process each run under their own. Stating one used
    /// to write it into the process, where the other session's reconnects
    /// found it.
    #[test]
    fn one_session_does_not_state_anothers() {
        let first = GatewaySettings {
            timezone: Some("America/New_York".to_string()),
            ..Default::default()
        }
        .resolve();
        let second = GatewaySettings {
            timezone: Some("Europe/Zurich".to_string()),
            ..Default::default()
        }
        .resolve();
        assert_eq!(first.timezone, "America/New_York");
        assert_eq!(second.timezone, "Europe/Zurich");
    }

    /// What the caller stated, else what the environment holds, else the
    /// default. A program configured the old way keeps working.
    #[test]
    fn the_environment_is_what_a_caller_states_nothing_over() {
        unsafe { std::env::set_var("IBX_LOCALE", "fr_FR") };
        let from_environment = GatewaySettings::default().resolve();
        assert_eq!(from_environment.locale, "fr_FR");
        // The identity this client announces with its locale set into it, read
        // from that identity rather than written out again: the two spellings
        // drifted apart the moment either changed.
        assert_eq!(
            from_environment.encoded,
            crate::config::IB_ENCODED.replace(crate::config::IB_LOCALE, "fr_FR"),
        );
        assert_ne!(from_environment.encoded, crate::config::IB_ENCODED);

        let stated = GatewaySettings {
            locale: Some("ja_JP".to_string()),
            ..Default::default()
        }
        .resolve();
        assert_eq!(stated.locale, "ja_JP", "the caller's own wins");
        unsafe { std::env::remove_var("IBX_LOCALE") };

        let neither = GatewaySettings::default().resolve();
        assert_eq!(neither.timezone, "UTC");
        assert_eq!(neither.build, crate::config::IB_BUILD);
        assert_eq!(neither.port, crate::config::MISC_PORT);
        assert!(neither.island_for_nasdaq, "the documented default");

        // However it is spelled. Compared against the lowercase spelling
        // alone, `Today` resolves to every execution the venue holds and
        // `False` leaves the older exchange spelling on, each the opposite of
        // what is written. Checked here rather than in a test of its own,
        // because these are the process's own variables and a second test
        // setting them races this one reading them.
        for spelling in ["today", "Today", "TODAY"] {
            unsafe { std::env::set_var("IBX_EXECUTION_REPORTS", spelling) };
            assert_eq!(
                GatewaySettings::default().resolve().execution_reports,
                ExecutionReportScope::Today,
                "{spelling} asked for every execution the venue holds",
            );
        }
        unsafe { std::env::set_var("IBX_EXECUTION_REPORTS", "yesterday") };
        assert_eq!(
            GatewaySettings::default().resolve().execution_reports,
            ExecutionReportScope::All,
            "a value naming neither keeps the default",
        );
        unsafe { std::env::remove_var("IBX_EXECUTION_REPORTS") };

        for spelling in ["false", "False", "NO", "0"] {
            unsafe { std::env::set_var("IBX_ISLAND_FOR_NASDAQ", spelling) };
            assert!(
                !GatewaySettings::default().resolve().island_for_nasdaq,
                "{spelling} left the older spelling on",
            );
        }
        unsafe { std::env::remove_var("IBX_ISLAND_FOR_NASDAQ") };
    }

    /// Every field of the stated form reaches the resolved one. A field added
    /// to one and not the other is a setting a caller can state and nothing
    /// reads.
    #[test]
    fn every_setting_is_resolved() {
        let all = GatewaySettings {
            timezone: Some("t".into()),
            locale: Some("l".into()),
            build: Some("b".into()),
            version: Some("v".into()),
            encoded: Some("e".into()),
            hardware_id: Some("h".into()),
            market_data_host: Some("m".into()),
            port: Some(1),
            registration_timeout_ms: Some(2),
            log_level: Some("debug".into()),
            log_dir: Some("d".into()),
            log_queue: Some(4096),
            execution_reports: Some(ExecutionReportScope::Today),
            island_for_nasdaq: Some(false),
        };
        let resolved = all.resolve();
        assert_eq!(resolved.timezone, "t");
        assert_eq!(resolved.locale, "l");
        assert_eq!(resolved.build, "b");
        assert_eq!(resolved.version, "v");
        assert_eq!(resolved.encoded, "e");
        assert_eq!(resolved.hardware_id.as_deref(), Some("h"));
        assert_eq!(resolved.market_data_host.as_deref(), Some("m"));
        assert_eq!(resolved.port, 1);
        assert_eq!(resolved.registration_timeout, std::time::Duration::from_millis(2));
        assert_eq!(resolved.execution_reports, ExecutionReportScope::Today);
        assert!(!resolved.island_for_nasdaq);
        // The three log settings are process-scoped by nature — one logger per
        // process — so they are not on the resolved form. They reach the
        // logger instead, and are checked here for the same reason: stated
        // and reaching neither, they were a setting a caller could state and
        // nothing read.
        let logging = crate::logging::LogConfig::stated(&all);
        assert_eq!(logging.level.as_deref(), Some("debug"));
        assert_eq!(logging.log_dir.as_deref(), Some(std::path::Path::new("d")));
        assert_eq!(logging.queue_capacity, 4096);

        // Destructured without `..`, so a field added to the stated form stops
        // compiling here until it is resolved above.
        let GatewaySettings {
            timezone: _, locale: _, build: _, version: _, encoded: _, hardware_id: _,
            market_data_host: _, port: _, registration_timeout_ms: _,
            log_level: _, log_dir: _, log_queue: _,
            execution_reports: _, island_for_nasdaq: _,
        } = all;
    }
}
