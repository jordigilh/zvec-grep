//go:build linux

package matrix

func platformTarget() {}
func platformOnlyCaller() { platformTarget() }
