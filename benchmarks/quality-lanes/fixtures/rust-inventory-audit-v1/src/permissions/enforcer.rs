use crate::permissions::policy::PolicySet;

/// Enforcer answers permission questions against the current PolicySet.
pub struct Enforcer {
    policies: PolicySet,
}

impl Enforcer {
    pub fn new(policies: PolicySet) -> Self {
        Self { policies }
    }

    /// Reports whether the named permission is currently granted.
    pub fn check_access(&self, name: &str) -> bool {
        self.policies.is_granted(name)
    }

    /// Returns an AccessDenied error when the permission is missing;
    /// Ok(()) when enforcement passes.
    pub fn require_permission(&self, name: &str) -> Result<(), AccessDenied> {
        if !self.check_access(name) {
            return Err(AccessDenied {
                permission: name.to_owned(),
            });
        }
        Ok(())
    }
}

/// AccessDenied is the fail-closed enforcement error.
#[derive(Debug)]
pub struct AccessDenied {
    pub permission: String,
}
