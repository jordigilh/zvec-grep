pub struct PermitLedger {
    accepted: Vec<String>,
}

impl PermitLedger {
    pub fn remember(&mut self, key: &str) {
        self.accepted.push(key.to_owned());
    }

    pub fn has(&self, key: &str) -> bool {
        self.accepted.iter().any(|candidate| candidate == key)
    }
}

pub fn route(ledger: &PermitLedger, key: &str) -> bool {
    ledger.has(key)
}
