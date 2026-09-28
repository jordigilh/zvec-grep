package matrix

import "fmt"

func platformCaller() { platformTarget() }
func taggedCaller()   { taggedTarget() }
func genericCaller()  { _ = identity[int](1) }
func externalCaller() { fmt.Println("external") }
