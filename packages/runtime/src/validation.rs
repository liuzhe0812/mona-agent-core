pub(crate) fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 64
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
