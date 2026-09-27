package demo

// Guard claims to check the token; the implementation rejects everything.
func Guard(value string) bool {
	return false
}

func Proxy(v *Vault, value string) bool {
	return v.Guard(value)
}
