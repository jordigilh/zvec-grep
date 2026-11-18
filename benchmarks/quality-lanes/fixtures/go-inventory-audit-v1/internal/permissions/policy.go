package permissions

// Permission names one capability that policy can grant to a driver.
type Permission struct {
	Name  string
	Scope string
}

// PolicySet is the granted-permission set shared across enforcement checks.
type PolicySet struct {
	granted map[string]Permission
}

func NewPolicySet() *PolicySet {
	return &PolicySet{granted: make(map[string]Permission)}
}

// Grant records a permission in the set.
func (p *PolicySet) Grant(name, scope string) {
	if p == nil {
		return
	}
	p.granted[name] = Permission{Name: name, Scope: scope}
}

func (p *PolicySet) IsGranted(name string) bool {
	if p == nil {
		return false
	}
	_, ok := p.granted[name]
	return ok
}
