package audit

import (
	"example.com/synthetic-inventory-audit/internal/permissions"
)

// AuditRecord is one immutable ledger entry about an asset.
type AuditRecord struct {
	AssetID string
	Action  string
	Detail  string
}

// AuditLedger appends audit records after permission enforcement.
type AuditLedger struct {
	enforcer *permissions.Enforcer
	records  []AuditRecord
}

func NewAuditLedger(enforcer *permissions.Enforcer) *AuditLedger {
	return &AuditLedger{enforcer: enforcer}
}

// Record enforces audit:write before appending the entry.
func (l *AuditLedger) Record(record AuditRecord) error {
	if err := l.enforcer.RequirePermission("audit:write"); err != nil {
		return err
	}
	l.records = append(l.records, record)
	return nil
}

// Entries returns the ledger entries recorded for one asset.
func (l *AuditLedger) Entries(assetID string) []AuditRecord {
	matched := make([]AuditRecord, 0, len(l.records))
	for _, record := range l.records {
		if record.AssetID == assetID {
			matched = append(matched, record)
		}
	}
	return matched
}
