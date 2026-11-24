package audit;

/**
 * One appended ledger entry describing an action taken on an asset.
 */
public record AuditRecord(String assetId, String action, String detail) {
}
