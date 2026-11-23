package permissions;

/**
 * Fail-closed permission gate over a PolicySet. requirePermission turns a
 * failed membership check into an AccessDenied error; checkAccess only
 * answers the membership question.
 */
public final class Enforcer {

    /** Fail-closed error raised when a required permission is missing. */
    public static final class AccessDenied extends RuntimeException {
        public AccessDenied(String permission) {
            super("permission not granted: " + permission);
        }
    }

    private final PolicySet policy;

    public Enforcer(PolicySet policy) {
        this.policy = policy;
    }

    public boolean checkAccess(String permission) {
        return policy.isGranted(permission);
    }

    public void requirePermission(String permission) {
        if (!checkAccess(permission)) {
            throw new AccessDenied(permission);
        }
    }
}
