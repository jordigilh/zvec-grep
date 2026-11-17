package demo

import "example.com/codegraph/internal/helper"

func UsesHelper() {
	helper.NewBase()
}

func AmbiguousCaller() {
	Load()
}
