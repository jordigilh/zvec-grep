package feature04

type Alpha struct{}
type Beta struct{}

func (Alpha) Convert(value int) int {
	return value + 1
}
func (Beta) Convert(value int) int {
	return value + 2
}
func AlphaCaller(a Alpha, value int) int {
	return a.Convert(value)
}
func BetaCaller(b Beta, value int) int {
	return b.Convert(value)
}

type Runner interface {
	Run(value int) int
}

type Worker struct{}

func (Worker) Run(value int) int {
	return value + 4
}
func InterfaceCaller(r Runner, value int) int {
	return r.Run(value)
}
func InterfaceCallerOuter(value int) int {
	return InterfaceCaller(Worker{}, value)
}

func Identity[T any](value T) T {
	return value
}
func GenericCaller(value int) int {
	return Identity(value)
}

func Dispatch(value int) int {
	return value
}
func ShadowedDispatchCaller(value int) int {
	Dispatch := func(value int) int { return value + 1 }
	return Dispatch(value)
}
func FunctionValueCaller(fn func(int) int, value int) int {
	return fn(value)
}
