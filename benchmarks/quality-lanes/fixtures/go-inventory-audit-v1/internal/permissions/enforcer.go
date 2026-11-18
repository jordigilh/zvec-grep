package permissions

// Enforcer answers permission questions against the current PolicySet.
type Enforcer struct {
	policies *PolicySet
}

func NewEnforcer(policies *PolicySet) *Enforcer {
	return &Enforcer{policies: policies}
}

// CheckAccess reports whether the named permission is currently granted.
func (e *Enforcer) CheckAccess(name string) bool {
	return e.policies.IsGranted(name)
}

// RequirePermission returns an AccessDenied error when the permission is
// missing; nil when enforcement passes.
func (e *Enforcer) RequirePermission(name string) error {
	if !e.CheckAccess(name) {
		return &AccessDenied{Permission: name}
	}
	return nil
}

// AccessDenied is the fail-closed enforcement error.
type AccessDenied struct {
	Permission string
}

func (e *AccessDenied) Error() string {
	return "access denied: missing permission " + e.Permission
}
