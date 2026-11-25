/// CDDL names as target-language identifiers (dgtlrift/cddlc#123).

use cddlc_codegen::{to_pascal_case, to_screaming_snake, to_snake_case};

#[test]
fn socket_prefixes_and_at_signs_are_dropped() {
    assert_eq!(to_pascal_case("$value"), "Value");
    assert_eq!(to_pascal_case("$$session-ext"), "SessionExt");
    assert_eq!(to_pascal_case("@owner"), "Owner");
    assert_eq!(to_snake_case("$value"), "value");
    assert_eq!(to_snake_case("$$session-ext"), "session_ext");
    assert_eq!(to_screaming_snake("@owner"), "OWNER");
}

#[test]
fn dots_separate_words_like_dashes() {
    assert_eq!(to_pascal_case("session.status"), "SessionStatus");
    assert_eq!(to_pascal_case("browsingContext.Activate"), "BrowsingContextActivate");
    assert_eq!(to_snake_case("session.status-code"), "session_status_code");
    assert_eq!(to_screaming_snake("session.status"), "SESSION_STATUS");
}

#[test]
fn plain_names_are_unchanged() {
    assert_eq!(to_pascal_case("sensor-reading"), "SensorReading");
    assert_eq!(to_snake_case("sensor-reading"), "sensor_reading");
    assert_eq!(to_snake_case("sensor_reading"), "sensor_reading");
}
