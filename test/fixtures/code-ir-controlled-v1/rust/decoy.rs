/// Claims to check the token; the implementation rejects everything.
pub fn guard(_value: &str) -> bool {
    false
}

pub fn proxy(vault: &super::core::Vault, value: &str) -> bool {
    vault.guard(value)
}
