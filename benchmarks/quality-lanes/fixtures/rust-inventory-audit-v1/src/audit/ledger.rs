use crate::permissions::enforcer::Enforcer;

/// AuditRecord is one immutable ledger entry about an asset.
pub struct AuditRecord {
    pub asset_id: String,
    pub action: String,
    pub detail: String,
}

/// AuditLedger appends audit records after permission enforcement.
pub struct AuditLedger {
    enforcer: Enforcer,
    records: Vec<AuditRecord>,
}

impl AuditLedger {
    pub fn new(enforcer: Enforcer) -> Self {
        Self {
            enforcer,
            records: Vec::new(),
        }
    }

    /// Enforces audit:write before appending the entry.
    pub fn record(&mut self, record: AuditRecord) -> Result<(), crate::permissions::enforcer::AccessDenied> {
        self.enforcer.require_permission("audit:write")?;
        self.records.push(record);
        Ok(())
    }

    /// Returns the ledger entries recorded for one asset.
    pub fn entries(&self, asset_id: &str) -> Vec<&AuditRecord> {
        self.records
            .iter()
            .filter(|record| record.asset_id == asset_id)
            .collect()
    }
}
