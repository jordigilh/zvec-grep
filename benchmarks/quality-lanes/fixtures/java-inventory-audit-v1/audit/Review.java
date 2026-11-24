package audit;

import inventory.ScanState;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/**
 * Review-side audit checks. The static checkAccess deliberately shares its
 * bare name with the permissions.Enforcer.checkAccess method so that a
 * name-only lookup must disambiguate by package.
 */
public final class Review {

    /** Review outcome for one asset. */
    public record ReviewFindings(String assetId, String severity, List<String> notes) {
    }

    private static final int REVIEW_BUDGET = 100;

    public static boolean checkAccess(Map<String, Object> context, AuditLedger ledger, String assetId) {
        ScanState state = ScanState.stateFromContext(context);
        if (!state.contains(assetId)) {
            return false;
        }
        return !ledger.entries(assetId).isEmpty();
    }

    public static Map<String, List<ReviewFindings>> findingsByAsset(List<AuditRecord> records) {
        Map<String, List<ReviewFindings>> byAsset = new LinkedHashMap<>();
        for (int i = 0; i < records.size() && i < REVIEW_BUDGET; i++) {
            AuditRecord entry = records.get(i);
            String severity = entry.action().equals("remediate") ? "warning" : "info";
            byAsset.computeIfAbsent(entry.assetId(), id -> new ArrayList<>())
                    .add(new ReviewFindings(entry.assetId(), severity, List.of(entry.detail())));
        }
        return byAsset;
    }

    public static Map<String, Integer> summarizeSeverity(List<ReviewFindings> findings) {
        Map<String, Integer> counts = new LinkedHashMap<>();
        for (int i = 0; i < findings.size() && i < REVIEW_BUDGET; i++) {
            counts.merge(findings.get(i).severity(), 1, Integer::sum);
        }
        return counts;
    }
}
