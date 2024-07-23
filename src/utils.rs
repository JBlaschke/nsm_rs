use std::time::{SystemTime, UNIX_EPOCH};

/// Vec must contain only one item or returns error
pub fn only_or_error(vec: & Vec<String>) -> & String {
    match vec.as_slice() {
        [element] => element,
        _ => panic!("Vector does not contain a single element"),
    }
}

/// Vec must contain one element or None for success
#[allow(unused)]
pub fn only_or_none(vec: & Vec<String>) -> Option<& String> {
    match vec.as_slice() {
        [element] => Some(element),
        _ => None,
    }
}

/// returns current epoch time
#[allow(unused)]
pub fn epoch() -> u64 {
    let now = SystemTime::now();
    let now_epoch = now.duration_since(UNIX_EPOCH).expect(
        "Failed to calculate duration"
    );

    now_epoch.as_secs()
}