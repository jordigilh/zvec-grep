package demo

func helper() {}

func TestChild() {
	helper()
}

func NegativeCaller() {
	Missing()
}
