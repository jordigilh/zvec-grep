class PermitLedger:
    def __init__(self) -> None:
        self.accepted: set[str] = set()

    def remember(self, key: str) -> None:
        self.accepted.add(key)

    def has(self, key: str) -> bool:
        return key in self.accepted


def route(ledger: PermitLedger, key: str) -> bool:
    return ledger.has(key)
