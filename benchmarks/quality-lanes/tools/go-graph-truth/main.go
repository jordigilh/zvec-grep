// Command graphtruth computes a compiler-validated (SSA + RTA) call graph
// for the frozen Go fixture and dumps every reachable call edge as JSON.
//
// Root policy: the fixture has no main package and no test files, so RTA is
// rooted at every function and method defined in the module's own packages
// (exported and unexported alike). This makes the graph a complete static
// call-relation over fixture-declared callables, which is the fairest
// compiler truth for adjudicating a name-mention scan.
package main

import (
	"encoding/json"
	"fmt"
	"go/ast"
	"go/token"
	"go/types"
	"os"
	"path/filepath"
	"sort"
	"strings"

	"golang.org/x/tools/go/callgraph"
	"golang.org/x/tools/go/callgraph/rta"
	"golang.org/x/tools/go/packages"
	"golang.org/x/tools/go/ssa"
	"golang.org/x/tools/go/ssa/ssautil"
)

var fixtureDir string

// Fixture directory, module path prefix, and output file are configurable so
// this copy can be reused verbatim for other fixtures (logic unchanged from
// go-graph-truth-output/tool/main.go).
func init() {
	dir := os.Getenv("GRAPHTRUTH_FIXTURE")
	if dir == "" {
		dir = "../work/fixture"
	}
	abs, err := filepath.Abs(dir)
	if err != nil {
		panic(err)
	}
	fixtureDir = abs
	modulePrefix = os.Getenv("GRAPHTRUTH_MODULE")
	if modulePrefix == "" {
		modulePrefix = "example.com/synthetic-workflow-discovery"
	}
	outPath = os.Getenv("GRAPHTRUTH_OUT")
	if outPath == "" {
		outPath = "../work/compiler-edges.json"
	}
}

var modulePrefix string
var outPath string

type Span struct {
	File      string `json:"file"`
	StartLine int    `json:"start_line"`
	EndLine   int    `json:"end_line"`
	StartByte int    `json:"start_byte"`
	EndByte   int    `json:"end_byte"`
}

type Node struct {
	Name     string `json:"name"`   // package-qualified, receiver-qualified for methods
	Symbol   string `json:"symbol"` // bare manifest symbol if the node maps to a manifest unit, else ""
	Kind     string `json:"kind"`   // func | method | type (never emitted for types) | synthetic
	PkgPath  string `json:"pkg_path"`
	RecvType string `json:"recv_type,omitempty"`
	Exported bool   `json:"exported"`
	Span     Span   `json:"span"`
}

type Edge struct {
	Caller   Node   `json:"caller"`
	Callee   Node   `json:"callee"`
	CallSite Span   `json:"call_site"`
	IsStatic bool   `json:"static_call"` // true = static (non-interface/dispatch) call site
	Desc     string `json:"description,omitempty"`
}

type Output struct {
	RootPolicy string `json:"root_policy"`
	EdgeCount  int    `json:"edge_count"`
	Edges      []Edge `json:"edges"`
}

func main() {
	fset := token.NewFileSet()
	cfg := &packages.Config{
		Dir:  fixtureDir,
		Mode: packages.NeedName | packages.NeedFiles | packages.NeedSyntax | packages.NeedTypes | packages.NeedTypesInfo | packages.NeedImports | packages.NeedDeps | packages.NeedModule,
		Fset: fset,
	}
	pkgs, err := packages.Load(cfg, "./...")
	if err != nil {
		die("load: %v", err)
	}
	var modPkgs []*packages.Package
	for _, p := range pkgs {
		if p.Errors != nil && len(p.Errors) > 0 {
			for _, e := range p.Errors {
				fmt.Fprintf(os.Stderr, "pkg error %s: %v\n", p.PkgPath, e)
			}
		}
		modPkgs = append(modPkgs, p)
	}
	sort.Slice(modPkgs, func(i, j int) bool { return modPkgs[i].PkgPath < modPkgs[j].PkgPath })

	// object spans: types.Object -> declaration span
	objSpan := map[types.Object]Span{}
	collectSpans := func(p *packages.Package) {
		for _, f := range p.Syntax {
			// relativize file path against fixture root
			posFile := fset.File(f.Pos())
			fname := posFile.Name()
			rel, err := filepath.Rel(fixtureDir, fname)
			if err != nil {
				rel = fname
			}
			ast.Inspect(f, func(n ast.Node) bool {
				var obj types.Object
				switch d := n.(type) {
				case *ast.FuncDecl:
					obj = p.TypesInfo.Defs[d.Name]
				case *ast.GenDecl:
					return true
				case *ast.TypeSpec:
					obj = p.TypesInfo.Defs[d.Name]
				}
				if obj != nil {
					objSpan[obj] = Span{
						File:      filepath.ToSlash(rel),
						StartLine: fset.Position(n.Pos()).Line,
						EndLine:   fset.Position(n.End()).Line,
						StartByte: posFile.Offset(n.Pos()),
						EndByte:   posFile.Offset(n.End()),
					}
				}
				return true
			})
		}
	}
	for _, p := range modPkgs {
		collectSpans(p)
	}

	prog, _ := ssautil.AllPackages(pkgs, ssa.InstantiateGenerics)
	prog.Build()

	// Roots: every function/method declared in module packages (no main, no tests).
	var roots []*ssa.Function
	nodeOf := map[*ssa.Function]*Node{}

	qualified := func(fn *ssa.Function) (name, recv, bare string) {
		pkgPath := ""
		if fn.Pkg != nil {
			pkgPath = fn.Pkg.Pkg.Path()
		}
		if r := fn.Signature.Recv(); r != nil {
			rt := types.Unalias(r.Type())
			if ptr, ok := rt.(*types.Pointer); ok {
				rt = ptr.Elem()
			}
			if named, ok := rt.(*types.Named); ok {
				recv = named.Obj().Name()
				name = pkgPath + ".(" + recv + ")." + fn.Name()
				bare = recv + "." + fn.Name()
				return
			}
		}
		name = pkgPath + "." + fn.Name()
		bare = fn.Name()
		return
	}

	newNode := func(fn *ssa.Function) *Node {
		if n, ok := nodeOf[fn]; ok {
			return n
		}
		name, recv, bare := qualified(fn)
		n := &Node{Name: name, Symbol: "", Kind: "func", PkgPath: "", RecvType: recv}
		if fn.Signature.Recv() != nil {
			n.Kind = "method"
		}
		if fn.Pkg != nil {
			n.PkgPath = fn.Pkg.Pkg.Path()
		}
		if fn.Object() != nil {
			n.Symbol = bare
			if sp, ok := objSpan[fn.Object()]; ok {
				n.Span = sp
			} else {
				pos := fn.Pos()
				if pos != token.NoPos {
					n.Span = spanOfPos(fset, pos)
				}
			}
			n.Exported = fn.Object().Exported()
		} else {
			n.Kind = "synthetic"
			n.Symbol = bare
			pos := fn.Pos()
			if pos != token.NoPos {
				n.Span = spanOfPos(fset, pos)
			}
		}
		nodeOf[fn] = n
		return n
	}

	inModule := func(fn *ssa.Function) bool {
		return fn.Pkg != nil && strings.HasPrefix(fn.Pkg.Pkg.Path(), modulePrefix)
	}

	for _, p := range modPkgs {
		if !strings.HasPrefix(p.PkgPath, modulePrefix) {
			continue
		}
		for _, obj := range p.TypesInfo.Defs {
			fobj, ok := obj.(*types.Func)
			if !ok || fobj.Pkg() == nil {
				continue
			}
			if fobj.Pkg().Path() != p.PkgPath {
				continue
			}
			if fn := prog.FuncValue(fobj); fn != nil && fn.Syntax() != nil {
				roots = append(roots, fn)
			}
		}
	}
	// dedupe roots
	seen := map[*ssa.Function]bool{}
	var uniqRoots []*ssa.Function
	for _, r := range roots {
		if !seen[r] {
			seen[r] = true
			uniqRoots = append(uniqRoots, r)
		}
	}
	roots = uniqRoots

	result := rta.Analyze(roots, true)
	cg := result.CallGraph

	var edges []Edge
	seenEdge := map[string]bool{}
	err = callgraph.GraphVisitEdges(cg, func(e *callgraph.Edge) error {
		caller, callee := e.Caller.Func, e.Callee.Func
		if !inModule(caller) || !inModule(callee) {
			return nil
		}
		key := fmt.Sprintf("%p|%p|%d", caller, callee, e.Pos())
		if seenEdge[key] {
			return nil
		}
		seenEdge[key] = true
		ed := Edge{
			Caller:   *newNode(caller),
			Callee:   *newNode(callee),
			IsStatic: strings.HasPrefix(e.Description(), "static"),
		}
		ed.Desc = e.Description()
		if e.Pos() != token.NoPos {
			ed.CallSite = spanOfPos(fset, e.Pos())
		}
		edges = append(edges, ed)
		return nil
	})
	if err != nil {
		die("visit: %v", err)
	}
	sort.Slice(edges, func(i, j int) bool {
		a, b := edges[i], edges[j]
		if a.Caller.Name != b.Caller.Name {
			return a.Caller.Name < b.Caller.Name
		}
		if a.Callee.Name != b.Callee.Name {
			return a.Callee.Name < b.Callee.Name
		}
		return a.CallSite.StartByte < b.CallSite.StartByte
	})

	out := Output{
		RootPolicy: "fixture has no main package and no _test.go files; RTA rooted at every function and method (exported and unexported) declared in the module's own packages",
		EdgeCount:  len(edges),
		Edges:      edges,
	}
	f, err := os.Create(outPath)
	if err != nil {
		die("create: %v", err)
	}
	defer f.Close()
	enc := json.NewEncoder(f)
	enc.SetIndent("", " ")
	if err := enc.Encode(&out); err != nil {
		die("encode: %v", err)
	}
	fmt.Printf("roots=%d edges=%d\n", len(roots), len(edges))
}

func spanOfPos(fset *token.FileSet, pos token.Pos) Span {
	p := fset.Position(pos)
	fl := fset.File(pos)
	sp := Span{File: p.Filename, StartLine: p.Line, EndLine: p.Line, StartByte: p.Offset, EndByte: p.Offset}
	if rel, err := filepath.Rel(fixtureDir, p.Filename); err == nil {
		sp.File = filepath.ToSlash(rel)
	}
	_ = fl
	return sp
}

func die(f string, args ...any) {
	fmt.Fprintf(os.Stderr, f+"\n", args...)
	os.Exit(1)
}
