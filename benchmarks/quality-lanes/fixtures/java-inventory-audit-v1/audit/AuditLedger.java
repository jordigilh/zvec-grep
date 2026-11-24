package audit;

import java.util.ArrayList;
import java.util.List;
import permissions.Enforcer;

/**
 * Append-only audit ledger. Every appended entry requires the audit:write
 * permission before it is stored.
 */
public final class AuditLedger {
    private final Enforcer enforcer;
    private final List<AuditRecord> entries = new ArrayList<>();

    public AuditLedger(Enforcer enforcer) {
        this.enforcer = enforcer;
    }

    public void append(AuditRecord entry) {
        enforcer.requirePermission("audit:write");
        entries.add(entry);
    }

    public List<AuditRecord> entries(String assetId) {
        return entries.stream()
                .filter(entry -> entry.assetId().equals(assetId))
                .toList();
    }
}
