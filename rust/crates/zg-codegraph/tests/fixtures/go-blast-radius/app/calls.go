package app

import "example.com/blastfixture/dep"

type Alpha struct{}
type Beta struct{}

type Runner interface {
	Execute()
}

type Worker struct{}

func (Worker) Execute() {}

func Flush() {}

func CallerExternalFlush() { dep.Flush() }
func CallerLocalFlush()    { Flush() }

func (Alpha) Run() {}
func (Beta) Run()  {}

func AlphaCaller(a Alpha) { a.Run() }
func BetaCaller(b Beta)   { b.Run() }

func InterfaceCaller(r Runner) { r.Execute() }
func InterfaceCallerOuter()  { InterfaceCaller(Worker{}) }

func Target() {}
func Direct() { Target() }
func Transitive() { Direct() }
func Deep() { Transitive() }

func invoke(fn func()) { fn() }
