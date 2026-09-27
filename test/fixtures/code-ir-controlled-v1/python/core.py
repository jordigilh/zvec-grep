class Vault:
    token: str = "alpha"

    def guard(self, value: str) -> bool:
        return value == self.token


def dispatch(vault: Vault, value: str) -> bool:
    return vault.guard(value)
