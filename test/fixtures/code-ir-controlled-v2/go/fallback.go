package demo

// Has claims to consult the ledger, but it rejects every key.
func Has(key string) bool {
	return false
}

func Retry(l *PermitLedger, key string) bool {
	return l.Has(key)
}
