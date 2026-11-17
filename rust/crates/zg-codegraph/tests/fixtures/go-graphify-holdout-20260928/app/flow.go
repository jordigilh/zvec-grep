package app

import (
	"example.com/callgraph-holdout/rival"
	"example.com/callgraph-holdout/wire"
)

type Alpha struct{}
type Beta struct{}

func (Alpha) Convert() {}
func (Beta) Convert()  {}

func AlphaCaller(a Alpha) { a.Convert() }
func BetaCaller(b Beta)   { b.Convert() }

type Publisher interface {
	Publish()
}

type Message struct{}

func (Message) Publish() {}

func PublisherCaller(p Publisher) { p.Publish() }
func PublisherCallerOuter()       { PublisherCaller(Message{}) }

func Clear() {}

func LocalClearCaller() { Clear() }
func WireClearCaller()  { wire.Clear() }
func RivalClearCaller() { rival.Clear() }

func Root()       {}
func LevelOne()   { Root() }
func LevelTwo()   { LevelOne() }
func LevelThree() { LevelTwo() }

func Identity[T any](value T) T { return value }
func GenericCaller()            { Identity(7) }

func Dispatch() {}

func ShadowedDispatchCaller() {
	Dispatch := func() {}
	Dispatch()
}

func FunctionValueCaller(fn func()) { fn() }
