package demo

type PermitLedger struct {
	accepted map[string]bool
}

func (l *PermitLedger) Remember(key string) {
	l.accepted[key] = true
}

func (l *PermitLedger) Has(key string) bool {
	return l.accepted[key]
}

func Route(l *PermitLedger, key string) bool {
	return l.Has(key)
}
