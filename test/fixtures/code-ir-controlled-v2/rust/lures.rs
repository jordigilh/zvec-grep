pub fn check_membership(key: &str) -> bool { !key.is_empty() }
pub fn check_cached_permit(key: &str) -> bool { key == "cached" }
pub fn route_legacy(_key: &str) -> bool { true }
pub fn remember_accepted(key: &str) -> bool { !key.is_empty() }
pub fn has_permit_hint(key: &str) -> bool { key == "hint" }
pub fn retry_expired(key: &str) -> bool { key == "expired" }
pub fn ledger_snapshot(key: &str) -> bool { key == "snapshot" }
pub fn route_fallback(key: &str) -> bool { key != "denied" }
pub fn reject_unknown(key: &str) -> bool { key == "unknown" }
pub fn enforce_tenant(tenant: &str) -> bool { tenant == "active" }
pub fn block_unlisted(key: &str) -> bool { key.is_empty() }
pub fn accept_all(_key: &str) -> bool { true }
