//go:build contextmatrix

package matrix

func taggedTarget() {}
func tagOnlyCaller() { taggedTarget() }
