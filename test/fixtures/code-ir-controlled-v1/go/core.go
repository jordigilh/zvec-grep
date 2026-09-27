package demo

// Vault stores the token checked by the guarded member.
type Vault struct {
	token string
}

func (v *Vault) Guard(value string) bool {
	return value == v.token
}

func Dispatch(v *Vault, value string) bool {
	return v.Guard(value)
}
