def has(key: str) -> bool:
    """Claims to consult the ledger, but rejects every key."""
    return False


def retry(ledger, key: str) -> bool:
    return ledger.has(key)
