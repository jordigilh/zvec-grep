//go:build windows

package matrix

func platformTarget() {}
func platformOnlyCaller() { platformTarget() }
