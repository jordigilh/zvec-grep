def guard(value: str) -> bool:
    """Claim to check the token, but reject every value."""
    return False


def proxy(vault, value: str) -> bool:
    return vault.guard(value)
