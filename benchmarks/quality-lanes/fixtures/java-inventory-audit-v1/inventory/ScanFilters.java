package inventory;

import java.util.LinkedHashMap;
import java.util.List;
import java.util.Locale;
import java.util.Map;

/**
 * Scan filters derived from harness signal tags. An empty field means
 * "not constrained"; populated fields must match asset tags.
 */
public record ScanFilters(String zone, String environment, String severity) {

    public static ScanFilters fromSignal(Map<String, String> tags) {
        return new ScanFilters(
                Normalize.term(tags.get("zone")),
                Normalize.term(tags.get("environment")),
                Normalize.term(tags.get("severity")));
    }

    public static List<Asset> applyTagFilters(List<Asset> assets, ScanFilters filters) {
        Map<String, String> expected = new LinkedHashMap<>();
        if (!filters.zone().isEmpty()) {
            expected.put("zone", filters.zone());
        }
        if (!filters.environment().isEmpty()) {
            expected.put("environment", filters.environment());
        }
        if (!filters.severity().isEmpty()) {
            expected.put("severity", filters.severity());
        }
        return assets.stream()
                .filter(asset -> expected.entrySet().stream().allMatch(entry ->
                        Normalize.term(asset.tags().getOrDefault(entry.getKey(), ""))
                                .equals(entry.getValue())))
                .toList();
    }
}

/** Filter-side term normalizer for scan tags. */
final class Normalize {
    static String term(String value) {
        return value == null ? "" : value.strip().toLowerCase(Locale.ROOT);
    }
}
