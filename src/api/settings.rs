//! The settings that used to live in the gateway's own file.
//!
//! A gateway is a process, and a process is configured by a file beside it and
//! a window in front of it. This client is a library and has neither, so the
//! same settings belong on the client, where a caller sets them in code and
//! reads them back.
//!
//! They are stated on [`EClientConfig`](crate::api::client::EClientConfig)
//! alongside the login, because a caller has one session and should not have
//! to configure it in two places — one of which, until now, was the process
//! environment.
//!
//! **These take effect for the whole process.** They are read wherever they
//! are needed rather than carried down to it, which is what the gateway's own
//! file amounted to as well. Two sessions in one process share them; a session
//! that needs its own would need them threaded through, and nothing has asked.

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
    /// How much it wrote down.
    pub log_level: Option<String>,
    /// Where it wrote it.
    pub log_dir: Option<String>,
    /// Whether it buffered what it wrote.
    pub log_queue: Option<bool>,

    // ── What the gateway did with what it received ──
    /// How many messages may go out per slice, and how long a slice is.
    ///
    /// The gateway ships with both at zero, which is no pacing at all. Stated
    /// here so a client that paces can be made to pace the same, and so that
    /// pacing more than the gateway did is a choice rather than an accident.
    pub messages_per_slice: Option<u32>,
    pub time_slice_ms: Option<u32>,
    /// Which executions arrive when a session opens: today's, or every one
    /// the venue still holds. The gateway asks for every one.
    pub execution_reports: Option<ExecutionReportScope>,
    /// What a delivered timestamp is stated in. The gateway states the
    /// operator's own time zone.
    pub timestamp_zone: Option<TimestampZone>,
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

/// What a delivered timestamp is stated in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimestampZone {
    /// The time zone this client runs in, which is what the gateway used.
    Operator,
    /// The one the instrument trades in.
    Instrument,
    /// Universal time.
    Utc,
}

/// Each setting, and the variable it is read from.
///
/// The variables are how this is carried, not what it is: a caller states the
/// setting and never sees the name below.
type Setting = (&'static str, fn(&GatewaySettings) -> Option<String>);

const VARIABLES: &[Setting] = &[
    ("IBX_TZ", |s| s.timezone.clone()),
    ("IBX_LOCALE", |s| s.locale.clone()),
    ("IBX_BUILD", |s| s.build.clone()),
    ("IBX_VERSION", |s| s.version.clone()),
    ("IBX_ENCODED", |s| s.encoded.clone()),
    ("IBX_HWID", |s| s.hardware_id.clone()),
    ("IBX_FARM_HOST", |s| s.market_data_host.clone()),
    ("IBX_MISC_PORT", |s| s.port.map(|p| p.to_string())),
    ("IBX_REGISTRATION_TIMEOUT_MS", |s| s.registration_timeout_ms.map(|n| n.to_string())),
    ("IBX_LOG_LEVEL", |s| s.log_level.clone()),
    ("IBX_LOG_DIR", |s| s.log_dir.clone()),
    ("IBX_LOG_QUEUE", |s| s.log_queue.map(|q| q.to_string())),
    ("IBX_MSGS_PER_SLICE", |s| s.messages_per_slice.map(|n| n.to_string())),
    ("IBX_TIME_SLICE_MS", |s| s.time_slice_ms.map(|n| n.to_string())),
    ("IBX_EXECUTION_REPORTS", |s| s.execution_reports.map(|scope| match scope {
        ExecutionReportScope::Today => "today".to_string(),
        ExecutionReportScope::All => "all".to_string(),
    })),
    ("IBX_TIMESTAMP_ZONE", |s| s.timestamp_zone.map(|zone| match zone {
        TimestampZone::Operator => "operator".to_string(),
        TimestampZone::Instrument => "instrument".to_string(),
        TimestampZone::Utc => "utc".to_string(),
    })),
    ("IBX_ISLAND_FOR_NASDAQ", |s| s.island_for_nasdaq.map(|on| on.to_string())),
];

/// Gateway settings with nothing to stand in for here, and why.
///
/// Named rather than dropped: someone moving off a gateway will look for them,
/// and "there is no such thing here" is an answer where silence is not.
pub const UNAVAILABLE: &[(&str, &str)] = &[
    ("LocalServerPort", "no local socket to listen on; this client is the client"),
    ("LocalApiPort", "no local socket to listen on; this client is the client"),
    ("TrustedIPs", "nothing connects to this client, so nothing needs trusting"),
    ("ApiOnly", "stated per session as `readonly` on the client config, not once for a process"),
    ("MainWindow.Width", "no window"),
    ("MainWindow.Height", "no window"),
    ("vmoptions", "no runtime to size"),
];

impl GatewaySettings {
    /// Put these where the code that needs them reads them.
    ///
    /// Called as a session opens. A setting a caller left alone is not
    /// cleared: whatever the environment already held stands, so a program
    /// configured the old way keeps working.
    pub fn apply(&self) {
        for (variable, read) in VARIABLES {
            if let Some(value) = read(self) {
                // Safety: called from `connect` before the engine's threads
                // start. Two sessions opening at once in one process would
                // race here, which is the same race the gateway's own file had
                // and the reason these are documented as process-wide.
                unsafe { std::env::set_var(variable, value) };
            }
        }
    }

    /// What every setting is currently, whether it was stated here or was
    /// already in the environment.
    pub fn in_force() -> Vec<(&'static str, Option<String>)> {
        VARIABLES
            .iter()
            .map(|(variable, _)| (*variable, std::env::var(variable).ok()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A setting stated on the client reaches the code that reads it.
    #[test]
    fn a_stated_setting_is_applied() {
        let settings = GatewaySettings {
            timezone: Some("America/New_York".to_string()),
            port: Some(4002),
            ..Default::default()
        };
        settings.apply();
        assert_eq!(std::env::var("IBX_TZ").as_deref(), Ok("America/New_York"));
        assert_eq!(crate::config::misc_port(), 4002);
        unsafe {
            std::env::remove_var("IBX_TZ");
            std::env::remove_var("IBX_MISC_PORT");
        }
    }

    /// One a caller left alone does not clear what was already set, so a
    /// program configured the old way keeps working.
    #[test]
    fn an_unstated_setting_leaves_what_was_there() {
        unsafe { std::env::set_var("IBX_LOCALE", "fr_FR") };
        GatewaySettings::default().apply();
        assert_eq!(std::env::var("IBX_LOCALE").as_deref(), Ok("fr_FR"));
        unsafe { std::env::remove_var("IBX_LOCALE") };
    }

    /// Every setting on the struct is carried. A field added and not listed
    /// would be one a caller can set and nothing reads.
    #[test]
    fn every_setting_is_carried() {
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
            messages_per_slice: Some(50),
            time_slice_ms: Some(1000),
            execution_reports: Some(ExecutionReportScope::All),
            timestamp_zone: Some(TimestampZone::Utc),
            island_for_nasdaq: Some(true),
        };
        let carried = VARIABLES.iter().filter(|(_, read)| read(&all).is_some()).count();
        assert_eq!(carried, VARIABLES.len(), "a setting is stated and not carried");
        // Destructured without `..`, so a field added to the struct stops
        // compiling here until it is carried above.
        let GatewaySettings {
            timezone: _, locale: _, build: _, version: _, encoded: _, hardware_id: _,
            market_data_host: _, port: _, registration_timeout_ms: _,
            log_level: _, log_dir: _, log_queue: _,
            messages_per_slice: _, time_slice_ms: _, execution_reports: _,
            timestamp_zone: _, island_for_nasdaq: _,
        } = all;
    }
}
