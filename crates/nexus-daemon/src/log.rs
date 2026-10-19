//! Thin helpers around the JSONL event log.

pub(crate) use nexus_events::EventLevel as Level;

pub(crate) fn log(logger: &nexus_events::EventLogger, level: Level, kind: &str, message: &str) {
    log_fields(
        logger,
        level,
        kind,
        message,
        std::iter::empty::<(&str, String)>(),
    );
}

pub(crate) fn log_fields<K: Into<String>>(
    logger: &nexus_events::EventLogger,
    level: Level,
    kind: &str,
    message: &str,
    fields: impl IntoIterator<Item = (K, String)>,
) {
    let fields = fields.into_iter().map(|(k, v)| (k.into(), v)).collect();
    if let Err(error) = logger.emit(level, kind, message, fields) {
        eprintln!("event log write failed: {error}");
    }
}
