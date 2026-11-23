package inventory;

import java.util.LinkedHashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;

/**
 * Shared per-investigation state holding the asset IDs that the scan
 * actually returned. Enforcement consults this state so that a registry
 * entry which was never scanned cannot be selected or audited.
 *
 * The set is insertion-ordered and only grows during one investigation.
 * An empty set is meaningful: it represents a scan that returned nothing
 * and therefore makes every registry ID fail membership checks.
 */
public final class ScanState {
    private final Set<String> ids = new LinkedHashSet<>();

    public void add(List<String> assetIds) {
        for (String assetId : assetIds) {
            if (assetId != null && !assetId.isBlank()) {
                ids.add(assetId);
            }
        }
    }

    public boolean contains(String assetId) {
        return ids.contains(assetId);
    }

    public static ScanState stateFromContext(Map<String, Object> context) {
        Object state = context.get("scanState");
        return state instanceof ScanState existing ? existing : new ScanState();
    }
}
