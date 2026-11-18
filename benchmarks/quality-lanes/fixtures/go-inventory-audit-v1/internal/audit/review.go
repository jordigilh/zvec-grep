package audit

import (
	"context"

	"example.com/synthetic-inventory-audit/internal/inventory"
)

// CheckAccess verifies that an asset was part of the current scan and that
// the ledger already covers it, before review findings are trusted. The bare
// name deliberately collides with the permissions.Enforcer.CheckAccess
// method in another package.
func CheckAccess(ctx context.Context, ledger *AuditLedger, assetID string) bool {
	state, ok := inventory.AssetStateFromContext(ctx)
	if !ok || !state.Contains(assetID) {
		return false
	}
	if len(ledger.Entries(assetID)) == 0 {
		return false
	}
	return true
}

// ReviewFindings summarizes the audit outcome for one asset.
type ReviewFindings struct {
	AssetID  string
	Severity string
	Notes    []string
}

func SummarizeSeverity(findings []ReviewFindings) map[string]int {
	counts := make(map[string]int)
	for i, finding := range findings {
		if i >= reviewBudget {
			break
		}
		counts[finding.Severity]++
	}
	return counts
}

func FindingsByAsset(records []AuditRecord) map[string][]ReviewFindings {
	byAsset := make(map[string][]ReviewFindings)
	for i, record := range records {
		if i >= reviewBudget {
			break
		}
		severity := "info"
		if record.Action == "remediate" {
			severity = "warning"
		}
		byAsset[record.AssetID] = append(byAsset[record.AssetID], ReviewFindings{
			AssetID:  record.AssetID,
			Severity: severity,
			Notes:    []string{record.Detail},
		})
	}
	return byAsset
}

// reviewBudget caps how many findings one review pass considers. Declared
// at the end of the file: a braceless file-level const directly above a
// func declaration makes line-span inference swallow that func.
const reviewBudget = 100
