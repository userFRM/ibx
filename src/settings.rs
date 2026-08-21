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
//! `log_queue` are not settled per session. Logging reads them from the
//! environment when it starts, under the names each field gives below, and a
//! value stated on the client is the name of the setting rather than a way to
//! set it.

/// A setting stated on the client, or left to the environment.
///
/// Every one of these stands in for something the gateway held. Where a caller
/// states nothing, what was already in the environment stands — so a program
/// configured the old way keeps working, and one configured in code does not
/// have to know the environment exists.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GatewaySettings {
    /// The time zone the gateway ran in, which is the one its times were
    /// stated in. Defaults to UTC.
    pub timezone: Option<String>,
    /// The locale it announced itself with.
    pub locale: Option<String>,
    /// The build it announced itself as, which the venue keeps a list of and
    /// stops accepting when it is old enough.
    pub build: Option<String>,
    /// The version beside that build.
    pub version: Option<String>,
    /// The longer string it announced with them.
    pub encoded: Option<String>,
    /// The machine identity it presented.
    pub hardware_id: Option<String>,
    /// The market data connection, where it is not the one the venue names.
    pub market_data_host: Option<String>,
    /// The port it reached the venue on.
    pub port: Option<u16>,
    /// How long it waited to be admitted, in milliseconds.
    pub registration_timeout_ms: Option<u64>,
    /// How much it wrote down. Logging reads this from `IBX_LOG_LEVEL`.
    pub log_level: Option<String>,
    /// Where it wrote it. Logging reads this from `IBX_LOG_DIR`.
    pub log_dir: Option<String>,
    /// Whether it buffered what it wrote. Logging reads this from
    /// `IBX_LOG_QUEUE`.
    pub log_queue: Option<bool>,

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

/// Gateway settings with nothing to stand in for here, and why.
///
/// Named rather than dropped: someone moving off a gateway will look for them,
/// and "there is no such thing here" is an answer where silence is not.
pub const UNAVAILABLE: &[(&str, &str)] = &[
    ("ApiMsgsPerSlice", "nothing here paces outgoing messages; the gateway ships with pacing off"),
    ("ApiTimeSliceMillis", "nothing here paces outgoing messages; the gateway ships with pacing off"),
    ("TimestampZone", "a timestamp is delivered as the venue states it, in the venue's terms"),
    ("LocalServerPort", "no local socket to listen on; this client is the client"),
    ("LocalApiPort", "no local socket to listen on; this client is the client"),
    ("TrustedIPs", "nothing connects to this client, so nothing needs trusting"),
    ("ApiOnly", "stated per session as `readonly` on the client config, not once for a process"),
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
    /// The zone the session states its times in.
    pub timezone: String,
    /// The locale it states them for.
    pub locale: String,
    /// The build this session announces itself as.
    pub build: String,
    /// The version it announces.
    pub version: String,
    /// What the session encodes its client string as.
    pub encoded: String,
    /// What it identifies this machine as. Derived when unset.
    pub hardware_id: Option<String>,
    /// Which market-data host to use, where the caller names one.
    pub market_data_host: Option<String>,
    /// Which port the session opens on.
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
            log_queue: Some(true),
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
        // Destructured without `..`, so a field added to the stated form stops
        // compiling here until it is resolved above. The three log settings
        // are process-scoped by nature — one logger per process — and are
        // named here rather than resolved.
        let GatewaySettings {
            timezone: _, locale: _, build: _, version: _, encoded: _, hardware_id: _,
            market_data_host: _, port: _, registration_timeout_ms: _,
            log_level: _, log_dir: _, log_queue: _,
            execution_reports: _, island_for_nasdaq: _,
        } = all;
    }
}
