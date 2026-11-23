package permissions;

import java.util.LinkedHashSet;
import java.util.Set;

/**
 * Grant set consulted by the enforcer. A permission name is either
 * granted or not; the set only grows during one investigation.
 */
public final class PolicySet {

    /** One granted permission, identified by its dotted name. */
    public record Permission(String name) {
    }

    private final Set<String> granted = new LinkedHashSet<>();

    public PolicySet(Set<String> names) {
        granted.addAll(names);
    }

    public void grant(Permission permission) {
        granted.add(permission.name());
    }

    public boolean isGranted(String name) {
        return granted.contains(name);
    }
}
