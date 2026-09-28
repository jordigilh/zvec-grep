package main

import (
	"bytes"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"flag"
	"fmt"
	"go/ast"
	"go/parser"
	"go/token"
	"go/types"
	"io/fs"
	"os"
	"path/filepath"
	"sort"
	"strings"

	"golang.org/x/tools/go/packages"
)

const (
	callFactsSchema  = "zvec-grep.go-callfacts"
	callFactsVersion = 1
	callFactsFile    = "go-callfacts-v1.json"
)

type CallFactsArtifact struct {
	Schema  string       `json:"schema"`
	Version int          `json:"version"`
	Files   []SourceFile `json:"files"`
	Calls   []CallFact   `json:"calls"`
}

type SourceFile struct {
	Path   string `json:"path"`
	SHA256 string `json:"sha256"`
}

type CallFact struct {
	Path            string   `json:"path"`
	StartByte       int      `json:"start_byte"`
	EndByte         int      `json:"end_byte"`
	StartLine       int      `json:"start_line"`
	EndLine         int      `json:"end_line"`
	StartColumn     int      `json:"start_column"`
	EndColumn       int      `json:"end_column"`
	Caller          string   `json:"caller"`
	TargetName      string   `json:"target_name"`
	Target          *string  `json:"target"`
	PossibleTargets []string `json:"possible_targets"`
	Resolution      string   `json:"resolution"`
}

func main() {
	rootFlag := flag.String("root", "", "Go workspace root")
	writeSidecar := flag.Bool("write-sidecar", false, "write .zvec-grep/go-callfacts-v1.json atomically")
	flag.Parse()
	if *rootFlag == "" {
		fatalf("--root is required")
	}
	root, err := filepath.Abs(*rootFlag)
	if err != nil {
		fatalf("resolve root: %v", err)
	}
	root, err = filepath.EvalSymlinks(root)
	if err != nil {
		fatalf("resolve root symlinks: %v", err)
	}
	artifact, err := buildCallFacts(root)
	if err != nil {
		fatalf("build Go call facts: %v", err)
	}
	if *writeSidecar {
		path := filepath.Join(root, ".zvec-grep", callFactsFile)
		if err := writeArtifactAtomically(path, artifact); err != nil {
			fatalf("write Go call-facts sidecar: %v", err)
		}
		fmt.Fprintf(os.Stderr, "Go call facts: %s (%d Go files, %d call sites)\n", path, len(artifact.Files), len(artifact.Calls))
		return
	}
	if err := json.NewEncoder(os.Stdout).Encode(artifact); err != nil {
		fatalf("encode Go call facts: %v", err)
	}
}

func buildCallFacts(root string) (CallFactsArtifact, error) {
	files, contents, err := collectSourceFiles(root)
	if err != nil {
		return CallFactsArtifact{}, err
	}
	artifact := CallFactsArtifact{
		Schema:  callFactsSchema,
		Version: callFactsVersion,
		Files:   files,
		Calls:   []CallFact{},
	}
	if len(files) == 0 {
		return artifact, nil
	}

	mode := packages.NeedName | packages.NeedFiles | packages.NeedCompiledGoFiles |
		packages.NeedSyntax | packages.NeedTypes | packages.NeedTypesInfo |
		packages.NeedImports | packages.NeedDeps
	loaded, err := packages.Load(&packages.Config{Mode: mode, Dir: root}, "./...")
	if err != nil {
		return CallFactsArtifact{}, fmt.Errorf("load Go workspace packages: %w", err)
	}
	if len(loaded) == 0 {
		return CallFactsArtifact{}, fmt.Errorf("Go workspace contains source files but no loadable packages")
	}

	packageByPath := make(map[string]*packages.Package)
	visited := make(map[*packages.Package]bool)
	var visit func(*packages.Package)
	visit = func(pkg *packages.Package) {
		if pkg == nil || visited[pkg] {
			return
		}
		visited[pkg] = true
		packageByPath[pkg.PkgPath] = pkg
		for _, imported := range pkg.Imports {
			visit(imported)
		}
	}
	for _, pkg := range loaded {
		visit(pkg)
		if pkg.IllTyped || len(pkg.Errors) > 0 {
			return CallFactsArtifact{}, packageTypeError(pkg)
		}
	}

	localPackages := make([]*packages.Package, 0, len(packageByPath))
	for _, pkg := range packageByPath {
		if packageIsWithin(root, pkg) {
			localPackages = append(localPackages, pkg)
		}
	}
	sort.Slice(localPackages, func(i, j int) bool { return localPackages[i].PkgPath < localPackages[j].PkgPath })

	coveredFiles := make(map[string]bool)
	for _, pkg := range loaded {
		facts, typedPaths, err := packageFacts(root, pkg, packageByPath, localPackages, contents)
		if err != nil {
			return CallFactsArtifact{}, err
		}
		artifact.Calls = append(artifact.Calls, facts...)
		for path := range typedPaths {
			coveredFiles[path] = true
		}
	}

	// Build-tagged, test-only, and otherwise inactive Go files remain in the
	// syntax graph. Emit unresolved facts for their calls so the opt-in overlay
	// cannot promote name-only guesses to resolved edges.
	for _, file := range files {
		if coveredFiles[file.Path] {
			continue
		}
		path := filepath.Join(root, filepath.FromSlash(file.Path))
		facts, err := syntaxOnlyFacts(root, path, contents[file.Path])
		if err != nil {
			return CallFactsArtifact{}, err
		}
		artifact.Calls = append(artifact.Calls, facts...)
	}
	if err := verifySourceFiles(root, files); err != nil {
		return CallFactsArtifact{}, err
	}
	sort.Slice(artifact.Calls, func(i, j int) bool {
		left, right := artifact.Calls[i], artifact.Calls[j]
		if left.Path != right.Path {
			return left.Path < right.Path
		}
		if left.StartByte != right.StartByte {
			return left.StartByte < right.StartByte
		}
		return left.Caller < right.Caller
	})
	return artifact, nil
}

func verifySourceFiles(root string, files []SourceFile) error {
	for _, file := range files {
		path := filepath.Join(root, filepath.FromSlash(file.Path))
		contents, err := os.ReadFile(path)
		if err != nil {
			return fmt.Errorf("recheck Go source %s: %w", file.Path, err)
		}
		digest := sha256.Sum256(contents)
		if got := hex.EncodeToString(digest[:]); got != file.SHA256 {
			return fmt.Errorf("Go source changed while call facts were generated: %s", file.Path)
		}
	}
	return nil
}

func packageTypeError(pkg *packages.Package) error {
	if len(pkg.Errors) == 0 {
		return fmt.Errorf("package %s is not fully type checked", pkg.PkgPath)
	}
	return fmt.Errorf("package %s: %s", pkg.PkgPath, pkg.Errors[0])
}

func collectSourceFiles(root string) ([]SourceFile, map[string][]byte, error) {
	var files []SourceFile
	contents := make(map[string][]byte)
	err := filepath.WalkDir(root, func(path string, entry fs.DirEntry, walkErr error) error {
		if walkErr != nil {
			return walkErr
		}
		if entry.IsDir() {
			if path != root && (entry.Name() == ".git" || entry.Name() == ".zvec-grep" || entry.Name() == "node_modules") {
				return filepath.SkipDir
			}
			return nil
		}
		if entry.Type()&fs.ModeSymlink != 0 || !entry.Type().IsRegular() || filepath.Ext(path) != ".go" {
			return nil
		}
		relative, err := filepath.Rel(root, path)
		if err != nil {
			return err
		}
		relative = filepath.ToSlash(relative)
		source, err := os.ReadFile(path)
		if err != nil {
			return err
		}
		digest := sha256.Sum256(source)
		files = append(files, SourceFile{Path: relative, SHA256: hex.EncodeToString(digest[:])})
		contents[relative] = source
		return nil
	})
	if err != nil {
		return nil, nil, fmt.Errorf("scan Go source files: %w", err)
	}
	sort.Slice(files, func(i, j int) bool { return files[i].Path < files[j].Path })
	return files, contents, nil
}

func packageIsWithin(root string, pkg *packages.Package) bool {
	for _, path := range pkg.GoFiles {
		if relativeToRoot(root, path) != "" {
			return true
		}
	}
	return false
}

func packageFacts(
	root string,
	pkg *packages.Package,
	packageByPath map[string]*packages.Package,
	localPackages []*packages.Package,
	contents map[string][]byte,
) ([]CallFact, map[string]bool, error) {
	var facts []CallFact
	typedPaths := make(map[string]bool)
	for _, file := range pkg.Syntax {
		absolute := pkg.Fset.Position(file.Pos()).Filename
		relative := relativeToRoot(root, absolute)
		if relative == "" {
			continue
		}
		source, ok := contents[relative]
		if !ok {
			return nil, nil, fmt.Errorf("Go package %s references untracked source %s", pkg.PkgPath, relative)
		}
		typedPaths[relative] = true

		var declarations []*ast.FuncDecl
		var calls []*ast.CallExpr
		ast.Inspect(file, func(node ast.Node) bool {
			switch node := node.(type) {
			case *ast.FuncDecl:
				if node.Body != nil {
					declarations = append(declarations, node)
				}
			case *ast.CallExpr:
				calls = append(calls, node)
			}
			return true
		})

		for _, call := range calls {
			declaration := nearestDeclaration(call, declarations)
			if declaration == nil {
				continue
			}
			fact, err := newCallFact(root, pkg.Fset, relative, pkg.Name, source, declaration, call)
			if err != nil {
				return nil, nil, err
			}
			callerObject, _ := pkg.TypesInfo.Defs[declaration.Name].(*types.Func)
			if callerObject == nil {
				fact.Resolution = "unresolved"
				facts = append(facts, fact)
				continue
			}
			fact.Caller = symbolKey(root, callerObject, packageByPath)
			if fact.Caller == "" {
				return nil, nil, fmt.Errorf("cannot map caller identity for %s:%d", relative, fact.StartLine)
			}
			_, resolution, target, possible := targetObject(root, pkg, call.Fun, localPackages, packageByPath)
			fact.Resolution = resolution
			fact.Target = target
			fact.PossibleTargets = possible
			if fact.Resolution == "static" && fact.Target == nil {
				fact.Resolution = "unresolved"
			}
			facts = append(facts, fact)
		}
	}
	return facts, typedPaths, nil
}

func syntaxOnlyFacts(root, path string, source []byte) ([]CallFact, error) {
	fset := token.NewFileSet()
	file, err := parser.ParseFile(fset, path, source, parser.AllErrors)
	if err != nil {
		return nil, fmt.Errorf("parse inactive Go source %s: %w", path, err)
	}
	relative := relativeToRoot(root, path)
	if relative == "" {
		return nil, fmt.Errorf("inactive Go source escaped workspace: %s", path)
	}
	var declarations []*ast.FuncDecl
	var calls []*ast.CallExpr
	ast.Inspect(file, func(node ast.Node) bool {
		switch node := node.(type) {
		case *ast.FuncDecl:
			if node.Body != nil {
				declarations = append(declarations, node)
			}
		case *ast.CallExpr:
			calls = append(calls, node)
		}
		return true
	})
	var facts []CallFact
	for _, call := range calls {
		declaration := nearestDeclaration(call, declarations)
		if declaration == nil {
			continue
		}
		fact, err := newCallFact(root, fset, relative, file.Name.Name, source, declaration, call)
		if err != nil {
			return nil, err
		}
		fact.Resolution = "unresolved"
		facts = append(facts, fact)
	}
	return facts, nil
}

func newCallFact(root string, fset *token.FileSet, path, packageName string, source []byte, declaration *ast.FuncDecl, call *ast.CallExpr) (CallFact, error) {
	start := fset.Position(call.Pos())
	end := fset.Position(call.End())
	funStart := fset.Position(call.Fun.Pos()).Offset
	funEnd := fset.Position(call.Fun.End()).Offset
	if start.Offset < 0 || end.Offset > len(source) || funStart < 0 || funEnd > len(source) || funStart > funEnd {
		return CallFact{}, fmt.Errorf("invalid Go token positions in %s:%d", path, start.Line)
	}
	caller := astSymbolKey(path, packageName, declaration)
	if caller == "" {
		return CallFact{}, fmt.Errorf("cannot derive caller identity for %s:%d", path, start.Line)
	}
	return CallFact{
		Path:            path,
		StartByte:       start.Offset,
		EndByte:         end.Offset,
		StartLine:       start.Line,
		EndLine:         end.Line,
		StartColumn:     max(start.Column-1, 0),
		EndColumn:       max(end.Column-1, 0),
		Caller:          caller,
		TargetName:      string(source[funStart:funEnd]),
		PossibleTargets: []string{},
		Resolution:      "unresolved",
	}, nil
}

func astSymbolKey(path, packageName string, declaration *ast.FuncDecl) string {
	if declaration == nil || declaration.Name == nil {
		return ""
	}
	if packageName == "" {
		return ""
	}
	name := packageName + "."
	if declaration.Recv != nil && len(declaration.Recv.List) > 0 {
		receiver := receiverNameFromAST(declaration.Recv.List[0].Type)
		if receiver == "" {
			return ""
		}
		name += receiver + "."
	}
	return path + "::" + name + declaration.Name.Name
}

func receiverNameFromAST(expression ast.Expr) string {
	switch expression := expression.(type) {
	case *ast.Ident:
		return expression.Name
	case *ast.StarExpr:
		return receiverNameFromAST(expression.X)
	case *ast.IndexExpr:
		return receiverNameFromAST(expression.X)
	case *ast.IndexListExpr:
		return receiverNameFromAST(expression.X)
	case *ast.SelectorExpr:
		return expression.Sel.Name
	default:
		return ""
	}
}

func nearestDeclaration(call *ast.CallExpr, declarations []*ast.FuncDecl) *ast.FuncDecl {
	var nearest *ast.FuncDecl
	var smallest int
	for _, declaration := range declarations {
		if declaration.Body.Pos() <= call.Pos() && call.End() <= declaration.Body.End() {
			size := int(declaration.Body.End() - declaration.Body.Pos())
			if nearest == nil || size < smallest {
				nearest = declaration
				smallest = size
			}
		}
	}
	return nearest
}

func targetObject(
	root string,
	pkg *packages.Package,
	expression ast.Expr,
	localPackages []*packages.Package,
	packageByPath map[string]*packages.Package,
) (types.Object, string, *string, []string) {
	var current ast.Expr = expression
	for {
		switch expression := current.(type) {
		case *ast.ParenExpr:
			current = expression.X
		case *ast.IndexExpr:
			current = expression.X
		case *ast.IndexListExpr:
			current = expression.X
		default:
			return targetObjectLeaf(root, pkg, current, localPackages, packageByPath)
		}
	}
}

func targetObjectLeaf(
	root string,
	pkg *packages.Package,
	expression ast.Expr,
	localPackages []*packages.Package,
	packageByPath map[string]*packages.Package,
) (types.Object, string, *string, []string) {
	var object types.Object
	switch expression := expression.(type) {
	case *ast.Ident:
		object = pkg.TypesInfo.Uses[expression]
	case *ast.SelectorExpr:
		if selection := pkg.TypesInfo.Selections[expression]; selection != nil {
			object = selection.Obj()
			if _, ok := object.(*types.Func); ok && isInterfaceType(selection.Recv()) {
				possible := interfaceTargets(root, selection.Recv(), object.Name(), localPackages, packageByPath)
				return object, "interface-dispatch", nil, possible
			}
		} else {
			object = pkg.TypesInfo.Uses[expression.Sel]
		}
	default:
		return nil, "unresolved", nil, []string{}
	}
	switch object := object.(type) {
	case *types.Func:
		key := symbolKey(root, object, packageByPath)
		if key == "" {
			return object, "external", nil, []string{}
		}
		return object, "static", &key, []string{}
	case *types.Builtin:
		return object, "external", nil, []string{}
	case *types.Var:
		if isFunctionType(object.Type()) {
			return object, "function-value", nil, []string{}
		}
		return object, "unresolved", nil, []string{}
	default:
		return object, "unresolved", nil, []string{}
	}
}

func isFunctionType(t types.Type) bool {
	_, ok := types.Unalias(t).Underlying().(*types.Signature)
	return ok
}

func isInterfaceType(t types.Type) bool {
	return t != nil && isUnderlyingInterface(types.Unalias(t).Underlying())
}

func isUnderlyingInterface(t types.Type) bool {
	_, ok := t.(*types.Interface)
	return ok
}

func interfaceTargets(
	root string,
	interfaceType types.Type,
	methodName string,
	localPackages []*packages.Package,
	packageByPath map[string]*packages.Package,
) []string {
	interfaceSet, ok := types.Unalias(interfaceType).Underlying().(*types.Interface)
	if !ok {
		return []string{}
	}
	interfaceSet = interfaceSet.Complete()
	var methodPackage *types.Package
	for index := 0; index < interfaceSet.NumMethods(); index++ {
		method := interfaceSet.Method(index)
		if method.Name() == methodName {
			methodPackage = method.Pkg()
			break
		}
	}
	if methodPackage == nil {
		return []string{}
	}

	possible := make(map[string]bool)
	for _, pkg := range localPackages {
		if pkg.Types == nil {
			continue
		}
		for _, name := range pkg.Types.Scope().Names() {
			typeName, ok := pkg.Types.Scope().Lookup(name).(*types.TypeName)
			if !ok {
				continue
			}
			named, ok := types.Unalias(typeName.Type()).(*types.Named)
			if !ok || isUnderlyingInterface(named.Underlying()) {
				continue
			}
			for _, candidateType := range []types.Type{named, types.NewPointer(named)} {
				if !types.Implements(candidateType, interfaceSet) {
					continue
				}
				selection := types.NewMethodSet(candidateType).Lookup(methodPackage, methodName)
				if selection == nil {
					continue
				}
				function, ok := selection.Obj().(*types.Func)
				if !ok {
					continue
				}
				key := symbolKey(root, function, packageByPath)
				if key != "" {
					possible[key] = true
				}
			}
		}
	}
	result := make([]string, 0, len(possible))
	for key := range possible {
		result = append(result, key)
	}
	sort.Strings(result)
	return result
}

func symbolKey(root string, function *types.Func, packageByPath map[string]*packages.Package) string {
	if function.Pkg() == nil {
		return ""
	}
	pkg := packageByPath[function.Pkg().Path()]
	if pkg == nil || pkg.Fset == nil {
		return ""
	}
	position := pkg.Fset.Position(function.Pos())
	if !position.IsValid() {
		return ""
	}
	relative := relativeToRoot(root, position.Filename)
	if relative == "" {
		return ""
	}
	name := pkg.Name + "."
	if signature, ok := function.Type().(*types.Signature); ok && signature.Recv() != nil {
		name += receiverName(signature.Recv().Type()) + "."
	}
	name += function.Name()
	return relative + "::" + name
}

func receiverName(t types.Type) string {
	t = types.Unalias(t)
	if pointer, ok := t.(*types.Pointer); ok {
		t = pointer.Elem()
	}
	if named, ok := t.(*types.Named); ok {
		return named.Obj().Name()
	}
	return types.TypeString(t, func(pkg *types.Package) string { return pkg.Name() })
}

func relativeToRoot(root, path string) string {
	absolute, err := filepath.Abs(path)
	if err != nil {
		return ""
	}
	relative, err := filepath.Rel(root, absolute)
	if err != nil || relative == "." || relative == ".." || strings.HasPrefix(relative, ".."+string(filepath.Separator)) {
		return ""
	}
	return filepath.ToSlash(relative)
}

func writeArtifactAtomically(path string, artifact CallFactsArtifact) error {
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return err
	}
	var contents bytes.Buffer
	encoder := json.NewEncoder(&contents)
	encoder.SetIndent("", "  ")
	if err := encoder.Encode(artifact); err != nil {
		return err
	}
	temporary, err := os.CreateTemp(filepath.Dir(path), ".go-callfacts-*.tmp")
	if err != nil {
		return err
	}
	temporaryName := temporary.Name()
	defer os.Remove(temporaryName)
	if _, err := temporary.Write(contents.Bytes()); err != nil {
		temporary.Close()
		return err
	}
	if err := temporary.Sync(); err != nil {
		temporary.Close()
		return err
	}
	if err := temporary.Close(); err != nil {
		return err
	}
	return os.Rename(temporaryName, path)
}

func fatalf(format string, arguments ...any) {
	fmt.Fprintf(os.Stderr, format+"\n", arguments...)
	os.Exit(1)
}
