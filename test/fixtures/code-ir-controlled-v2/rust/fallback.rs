/// Claims to consult the ledger, but rejects every key.
pub fn has(_key: &str) -> bool {
    false
}

pub fn retry(ledger: &crate::policy::PermitLedger, key: &str) -> bool {
    ledger.has(key)
}
