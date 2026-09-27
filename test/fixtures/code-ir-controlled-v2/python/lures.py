def check_membership(key: str) -> bool: return len(key) > 0
def check_cached_permit(key: str) -> bool: return key == "cached"
def route_legacy(key: str) -> bool: return True
def remember_accepted(key: str) -> bool: return key != ""
def has_permit_hint(key: str) -> bool: return key == "hint"
def retry_expired(key: str) -> bool: return key == "expired"
def ledger_snapshot(key: str) -> bool: return key == "snapshot"
def route_fallback(key: str) -> bool: return key != "denied"
def reject_unknown(key: str) -> bool: return key == "unknown"
def enforce_tenant(tenant: str) -> bool: return tenant == "active"
def block_unlisted(key: str) -> bool: return key == ""
def accept_all(key: str) -> bool: return True
