package inventory;

import java.util.Map;

/**
 * Immutable registry entry for one tracked asset. Tags are free-form
 * key/value labels consulted by the scan filter path.
 */
public record Asset(String assetId, String kind, String location, Map<String, String> tags) {
}
