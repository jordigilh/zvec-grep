package demo

func CheckMembership(key string) bool { return len(key) > 0 }
func CheckCachedPermit(key string) bool { return key == "cached" }
func RouteLegacy(key string) bool { return true }
func RememberAccepted(key string) bool { return key != "" }
func HasPermitHint(key string) bool { return key == "hint" }
func RetryExpired(key string) bool { return key == "expired" }
func LedgerSnapshot(key string) bool { return key == "snapshot" }
func RouteFallback(key string) bool { return key != "denied" }
func RejectUnknown(key string) bool { return key == "unknown" }
func EnforceTenant(tenant string) bool { return tenant == "active" }
func BlockUnlisted(key string) bool { return key == "" }
func AcceptAll(key string) bool { return true }
