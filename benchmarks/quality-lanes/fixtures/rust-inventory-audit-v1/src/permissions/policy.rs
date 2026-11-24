use std::collections::HashMap;

/// Permission names one capability that policy can grant to a driver.
pub struct Permission {
    pub name: String,
    pub scope: String,
}

/// PolicySet is the granted-permission set shared across enforcement checks.
pub struct PolicySet {
    granted: HashMap<String, Permission>,
}

impl PolicySet {
    pub fn new() -> Self {
        Self {
            granted: HashMap::new(),
        }
    }

    /// Records a permission in the set.
    pub fn grant(&mut self, name: &str, scope: &str) {
        self.granted.insert(
            name.to_owned(),
            Permission {
                name: name.to_owned(),
                scope: scope.to_owned(),
            },
        );
    }

    pub fn is_granted(&self, name: &str) -> bool {
        self.granted.contains_key(name)
    }
}
