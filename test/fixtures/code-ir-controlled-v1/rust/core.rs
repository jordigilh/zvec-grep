pub struct Vault {
    token: String,
}

impl Vault {
    pub fn guard(&self, value: &str) -> bool {
        value == self.token
    }
}

pub fn dispatch(vault: &Vault, value: &str) -> bool {
    vault.guard(value)
}
