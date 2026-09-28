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
	"hash"
	"io/fs"
	"os"
	"os/exec"
	"path/filepath"
	"sort"
	"strings"

	"golang.org/x/mod/modfile"
	"golang.org/x/tools/go/packages"
)

const (
	callFactsSchema  = "zvec-grep.go-callfacts"
	callFactsVersion = 2
	callFactsFile    = "go-callfacts-v2.json"
)

type CallFactsArtifact struct {
	Schema        string            `json:"schema"`
	Version       int               `json:"version"`
	Context       GoAnalysisContext `json:"context"`
	ContextSHA256 string            `json:"context_sha256"`
	Files         []SourceFile      `json:"files"`
	Calls         []CallFact        `json:"calls"`
}

type SourceFile struct {
	Path   string `json:"path"`
	SHA256 string `json:"sha256"`
}

// GoAnalysisContext records the effective build configuration and the
// in-workspace module/workspace files used to generate these facts. Facts are
// scoped to this recorded context; consumers without Go do not infer that a
// different active Go environment is equivalent.
type GoAnalysisContext struct {
	GoVersion    string            `json:"go_version"`
	GoMod        string            `json:"go_mod"`
	GoWork       string            `json:"go_work"`
	Settings     map[string]string `json:"settings"`
	ContextFiles []SourceFile      `json:"context_files"`
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
	writeSidecar := flag.Bool("write-sidecar", false, "write .zvec-grep/go-callfacts-v2.json atomically")
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
		fmt.Fprintf(os.Stderr, "Go call facts: %s (%d Go files, %d call sites; context %s)\n", path, len(artifact.Files), len(artifact.Calls), artifact.ContextSHA256)
		return
	}
	if err := json.NewEncoder(os.Stdout).Encode(artifact); err != nil {
		fatalf("encode Go call facts: %v", err)
	}
}

func buildCallFacts(root string) (CallFactsArtifact, error) {
	absoluteRoot, err := filepath.Abs(root)
	if err != nil {
		return CallFactsArtifact{}, fmt.Errorf("resolve Go workspace root: %w", err)
	}
	root, err = filepath.EvalSymlinks(absoluteRoot)
	if err != nil {
		return CallFactsArtifact{}, fmt.Errorf("resolve Go workspace root symlinks: %w", err)
	}
	files, contents, err := collectSourceFiles(root)
	if err != nil {
		return CallFactsArtifact{}, err
	}
	context, err := captureGoAnalysisContext(root, len(files) > 0)
	if err != nil {
		return CallFactsArtifact{}, err
	}
	artifact := CallFactsArtifact{
		Schema:        callFactsSchema,
		Version:       callFactsVersion,
		Context:       context,
		ContextSHA256: goAnalysisContextSHA256(context),
		Files:         files,
		Calls:         []CallFact{},
	}
	if len(files) == 0 {
		return artifact, nil
	}

	mode := packages.NeedName | packages.NeedFiles | packages.NeedCompiledGoFiles |
		packages.NeedSyntax | packages.NeedTypes | packages.NeedTypesInfo |
		packages.NeedImports | packages.NeedDeps
	if os.Getenv("GOPACKAGESDRIVER") != "" {
		return CallFactsArtifact{}, fmt.Errorf("GOPACKAGESDRIVER is unsupported; unset it to use the standard go list driver")
	}
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
	for _, pkg := range packageByPath {
		if packageUsesCgo(pkg.Syntax) {
			return CallFactsArtifact{}, fmt.Errorf("cgo-dependent packages are unsupported for Go call-facts generation: %s", pkg.PkgPath)
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
	currentContext, err := captureGoAnalysisContext(root, true)
	if err != nil {
		return CallFactsArtifact{}, err
	}
	if goAnalysisContextSHA256(currentContext) != artifact.ContextSHA256 {
		return CallFactsArtifact{}, fmt.Errorf("Go analysis context changed while call facts were generated")
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

var goContextSettingNames = []string{
	"GO111MODULE",
	"GO386",
	"GOAMD64",
	"GOARCH",
	"GOARM",
	"GOARM64",
	"CGO_ENABLED",
	"GOEXPERIMENT",
	"GOFLAGS",
	"GOMIPS",
	"GOMIPS64",
	"GOOS",
	"GOPPC64",
	"GORISCV64",
	"GOTOOLCHAIN",
	"GOWASM",
}

func captureGoAnalysisContext(root string, requireModule bool) (GoAnalysisContext, error) {
	command := exec.Command("go", "env", "-json")
	command.Dir = root
	output, err := command.Output()
	if err != nil {
		return GoAnalysisContext{}, fmt.Errorf("capture effective Go environment: %w", err)
	}
	var environment map[string]string
	if err := json.Unmarshal(output, &environment); err != nil {
		return GoAnalysisContext{}, fmt.Errorf("decode effective Go environment: %w", err)
	}
	if driver := os.Getenv("GOPACKAGESDRIVER"); driver != "" {
		return GoAnalysisContext{}, fmt.Errorf("GOPACKAGESDRIVER is unsupported; unset it to use the standard go list driver")
	}
	if err := validateGoFlags(environment["GOFLAGS"]); err != nil {
		return GoAnalysisContext{}, err
	}

	contextFiles, err := collectGoContextFiles(root)
	if err != nil {
		return GoAnalysisContext{}, err
	}
	context := GoAnalysisContext{
		GoVersion:    environment["GOVERSION"],
		Settings:     make(map[string]string, len(goContextSettingNames)),
		ContextFiles: contextFiles,
	}
	for _, name := range goContextSettingNames {
		context.Settings[name] = environment[name]
	}
	if context.GoVersion == "" || context.Settings["GOOS"] == "" || context.Settings["GOARCH"] == "" {
		return GoAnalysisContext{}, fmt.Errorf("Go environment is missing GOVERSION, GOOS, or GOARCH")
	}

	goMod := environment["GOMOD"]
	if goMod != "" && goMod != os.DevNull && !strings.EqualFold(goMod, "NUL") {
		context.GoMod, err = contextPathWithinRoot(root, goMod, "go.mod")
		if err != nil {
			return GoAnalysisContext{}, err
		}
	} else if requireModule {
		return GoAnalysisContext{}, fmt.Errorf("Go call facts require a go.mod within the workspace root")
	}

	goWork := environment["GOWORK"]
	switch goWork {
	case "", "off":
		context.GoWork = goWork
	default:
		context.GoWork, err = contextPathWithinRoot(root, goWork, "go.work")
		if err != nil {
			return GoAnalysisContext{}, err
		}
	}

	if context.GoMod != "" && !hasContextFile(context.ContextFiles, context.GoMod) {
		return GoAnalysisContext{}, fmt.Errorf("active module file %s is not in the Go context input set", context.GoMod)
	}
	if context.GoWork != "" && context.GoWork != "off" && !hasContextFile(context.ContextFiles, context.GoWork) {
		return GoAnalysisContext{}, fmt.Errorf("active workspace file %s is not in the Go context input set", context.GoWork)
	}
	if err := validateLocalModuleInputs(root, context.ContextFiles); err != nil {
		return GoAnalysisContext{}, err
	}
	return context, nil
}

func contextPathWithinRoot(root, path, kind string) (string, error) {
	absolute, err := filepath.Abs(path)
	if err != nil {
		return "", fmt.Errorf("resolve active %s %s: %w", kind, path, err)
	}
	canonical, err := filepath.EvalSymlinks(absolute)
	if err != nil {
		return "", fmt.Errorf("resolve active %s %s: %w", kind, path, err)
	}
	relative := relativeToRoot(root, canonical)
	if relative == "" {
		return "", fmt.Errorf("active %s is outside the Go workspace root: %s", kind, path)
	}
	return relative, nil
}

func collectGoContextFiles(root string) ([]SourceFile, error) {
	var files []SourceFile
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
		if entry.Type()&fs.ModeSymlink != 0 || !entry.Type().IsRegular() {
			return nil
		}
		relative, err := filepath.Rel(root, path)
		if err != nil {
			return err
		}
		if !isGoContextInputPath(relative) {
			return nil
		}
		contents, err := os.ReadFile(path)
		if err != nil {
			return err
		}
		digest := sha256.Sum256(contents)
		files = append(files, SourceFile{
			Path:   filepath.ToSlash(relative),
			SHA256: hex.EncodeToString(digest[:]),
		})
		return nil
	})
	if err != nil {
		return nil, fmt.Errorf("scan Go module/workspace inputs: %w", err)
	}
	sort.Slice(files, func(i, j int) bool { return files[i].Path < files[j].Path })
	return files, nil
}

func isGoContextInputPath(path string) bool {
	switch filepath.Base(path) {
	case "go.mod", "go.sum", "go.work", "go.work.sum":
		return true
	case "modules.txt":
		return filepath.Base(filepath.Dir(path)) == "vendor"
	default:
		return false
	}
}

func validateLocalModuleInputs(root string, files []SourceFile) error {
	for _, file := range files {
		path := filepath.Join(root, filepath.FromSlash(file.Path))
		contents, err := os.ReadFile(path)
		if err != nil {
			return fmt.Errorf("read Go context input %s: %w", file.Path, err)
		}
		switch filepath.Base(path) {
		case "go.mod":
			parsed, err := modfile.Parse(path, contents, nil)
			if err != nil {
				return fmt.Errorf("parse Go module input %s: %w", file.Path, err)
			}
			for _, replacement := range parsed.Replace {
				if replacement.New.Version == "" {
					if err := validateLocalPath(root, filepath.Dir(path), replacement.New.Path, "local module replacement"); err != nil {
						return err
					}
				}
			}
		case "go.work":
			parsed, err := modfile.ParseWork(path, contents, nil)
			if err != nil {
				return fmt.Errorf("parse Go workspace input %s: %w", file.Path, err)
			}
			for _, use := range parsed.Use {
				if err := validateLocalPath(root, filepath.Dir(path), use.Path, "workspace module"); err != nil {
					return err
				}
			}
			for _, replacement := range parsed.Replace {
				if replacement.New.Version == "" {
					if err := validateLocalPath(root, filepath.Dir(path), replacement.New.Path, "workspace replacement"); err != nil {
						return err
					}
				}
			}
		}
	}
	return nil
}

func validateLocalPath(root, base, path, description string) error {
	if !filepath.IsAbs(path) {
		path = filepath.Join(base, path)
	}
	canonical, err := filepath.EvalSymlinks(path)
	if err != nil {
		return fmt.Errorf("resolve %s %s: %w", description, path, err)
	}
	if !pathWithinRoot(root, canonical) {
		return fmt.Errorf("%s is outside the Go workspace root: %s", description, path)
	}
	return nil
}

func pathWithinRoot(root, path string) bool {
	relative, err := filepath.Rel(root, path)
	return err == nil && relative != ".." && !strings.HasPrefix(relative, ".."+string(filepath.Separator))
}

func hasContextFile(files []SourceFile, path string) bool {
	for _, file := range files {
		if file.Path == path {
			return true
		}
	}
	return false
}

func goAnalysisContextSHA256(context GoAnalysisContext) string {
	hasher := sha256.New()
	writeContextPart(hasher, "zvec-grep.go-callfacts-context-v1")
	writeContextPart(hasher, context.GoVersion)
	writeContextPart(hasher, context.GoMod)
	writeContextPart(hasher, context.GoWork)
	settingNames := make([]string, 0, len(context.Settings))
	for name := range context.Settings {
		settingNames = append(settingNames, name)
	}
	sort.Strings(settingNames)
	for _, name := range settingNames {
		writeContextPart(hasher, name)
		writeContextPart(hasher, context.Settings[name])
	}
	contextFiles := append([]SourceFile(nil), context.ContextFiles...)
	sort.Slice(contextFiles, func(i, j int) bool { return contextFiles[i].Path < contextFiles[j].Path })
	for _, file := range contextFiles {
		writeContextPart(hasher, file.Path)
		writeContextPart(hasher, file.SHA256)
	}
	return hex.EncodeToString(hasher.Sum(nil))
}

func writeContextPart(hasher hash.Hash, value string) {
	_, _ = hasher.Write([]byte(value))
	_, _ = hasher.Write([]byte{0})
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

func packageUsesCgo(syntax []*ast.File) bool {
	for _, file := range syntax {
		for _, importSpec := range file.Imports {
			if importSpec.Path != nil && importSpec.Path.Value == `"C"` {
				return true
			}
		}
	}
	return false
}

func validateGoFlags(flags string) error {
	unsupported := []string{"-overlay", "-modfile", "-toolexec", "-pkgdir"}
	for _, token := range strings.Fields(flags) {
		token = strings.Trim(token, `"'`)
		for _, flag := range unsupported {
			if token == flag || strings.HasPrefix(token, flag+"=") {
				return fmt.Errorf("GOFLAGS option %s is unsupported for source/context-attested call facts", flag)
			}
		}
	}
	return nil
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
