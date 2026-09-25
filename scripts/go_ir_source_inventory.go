// go_ir_source_inventory emits an independent Go-AST source-unit inventory for
// the frozen Go qeval fixture. It reads the manifest and source only; it does
// not consume SCIP or Code IR output.
//
//	go run scripts/go_ir_source_inventory.go --fixture PATH [--output PATH]
package main

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"flag"
	"fmt"
	"go/ast"
	"go/parser"
	"go/token"
	"os"
	"path/filepath"
	"runtime"
	"sort"
	"strings"
)

type manifest struct {
	FixtureID string `json:"fixture_id"`
	Language  string `json:"language"`
	Source    struct {
		Repository string   `json:"repository"`
		Include    []string `json:"include"`
		Exclude    []string `json:"exclude"`
	} `json:"source"`
	Units []manifestUnit `json:"units"`
}

type manifestUnit struct {
	ID     string `json:"unit_id"`
	Path   string `json:"path"`
	Symbol string `json:"symbol"`
}

type unitRecord struct {
	ID                  string `json:"unit_id"`
	Path                string `json:"path"`
	Symbol              string `json:"symbol"`
	Kind                string `json:"kind"`
	ASTNode             string `json:"ast_node"`
	StartByte           int    `json:"start_byte"`
	EndByte             int    `json:"end_byte"`
	IdentifierStartByte int    `json:"identifier_start_byte"`
	IdentifierEndByte   int    `json:"identifier_end_byte"`
	StartLine           int    `json:"start_line"`
	EndLine             int    `json:"end_line"`
}

type output struct {
	Schema       string       `json:"schema"`
	FixtureID    string       `json:"fixture_id"`
	Language     string       `json:"language"`
	Parser       string       `json:"parser"`
	Method       string       `json:"method"`
	SourceSHA256 string       `json:"source_set_sha256"`
	SourceFiles  int          `json:"source_files"`
	SourceBytes  int          `json:"source_bytes"`
	Units        []unitRecord `json:"units"`
}

type astDeclaration struct {
	unitSymbol string
	kind       string
	node       string
	start      token.Pos
	end        token.Pos
	nameStart  token.Pos
	nameEnd    token.Pos
}

func main() {
	fixture := flag.String("fixture", "", "frozen Go fixture directory")
	outputPath := flag.String("output", "", "optional JSON output path (stdout by default)")
	flag.Parse()
	if *fixture == "" {
		fatalf("--fixture is required")
	}
	if err := run(*fixture, *outputPath); err != nil {
		fatalf("%v", err)
	}
}

func run(fixture, outputPath string) error {
	fixture, err := filepath.Abs(fixture)
	if err != nil {
		return err
	}
	manifestBytes, err := os.ReadFile(filepath.Join(fixture, "manifest.json"))
	if err != nil {
		return err
	}
	var m manifest
	if err := json.Unmarshal(manifestBytes, &m); err != nil {
		return err
	}
	if m.Language != "go" || m.FixtureID == "" || len(m.Units) == 0 {
		return fmt.Errorf("expected a non-empty Go fixture manifest")
	}

	paths := map[string]struct{}{}
	for _, unit := range m.Units {
		paths[unit.Path] = struct{}{}
	}
	orderedPaths := make([]string, 0, len(paths))
	for path := range paths {
		orderedPaths = append(orderedPaths, path)
	}
	sort.Strings(orderedPaths)

	fileSet := token.NewFileSet()
	declarations := map[string][]astDeclaration{}
	selectedDigest := sha256.New()
	totalBytes := 0
	for _, relative := range orderedPaths {
		if filepath.IsAbs(relative) || strings.Contains(relative, "\\") || containsDotDot(relative) {
			return fmt.Errorf("unsafe fixture source path %q", relative)
		}
		path := filepath.Join(fixture, filepath.FromSlash(relative))
		source, err := os.ReadFile(path)
		if err != nil {
			return err
		}
		selectedDigest.Write([]byte(relative))
		selectedDigest.Write([]byte{0})
		selectedDigest.Write(source)
		selectedDigest.Write([]byte{0})
		totalBytes += len(source)

		file, err := parser.ParseFile(fileSet, path, source, parser.ParseComments|parser.AllErrors)
		if err != nil {
			return fmt.Errorf("parse %s: %w", relative, err)
		}
		for _, declaration := range file.Decls {
			switch node := declaration.(type) {
			case *ast.FuncDecl:
				kind, symbol := "function", node.Name.Name
				if node.Recv != nil && len(node.Recv.List) > 0 {
					kind = "method"
					receiver := receiverName(node.Recv.List[0].Type)
					if receiver == "" {
						return fmt.Errorf("unsupported receiver in %s at %s", relative, fileSet.Position(node.Recv.Pos()))
					}
					symbol = receiver + "." + symbol
				}
				declarations[relative] = append(declarations[relative], astDeclaration{
					unitSymbol: symbol, kind: kind, node: "FuncDecl",
					start: node.Pos(), end: node.End(),
					nameStart: node.Name.Pos(), nameEnd: node.Name.End(),
				})
			case *ast.GenDecl:
				for _, spec := range node.Specs {
					switch item := spec.(type) {
					case *ast.TypeSpec:
						start, end := item.Pos(), item.End()
						// Code IR v1.5 represents a single-spec Go type declaration as
						// its full `type` declaration, including attached comments. A
						// grouped declaration remains a source-backed TypeSpec.
						if len(node.Specs) == 1 {
							start, end = node.Pos(), node.End()
							if node.Doc != nil {
								start = node.Doc.Pos()
							}
						}
						declarations[relative] = append(declarations[relative], astDeclaration{
							unitSymbol: item.Name.Name, kind: "type", node: "TypeSpec",
							start: start, end: end,
							nameStart: item.Name.Pos(), nameEnd: item.Name.End(),
						})
					case *ast.ValueSpec:
						kind := "value"
						if node.Tok == token.VAR || node.Tok == token.CONST {
							for _, name := range item.Names {
								declarations[relative] = append(declarations[relative], astDeclaration{
									unitSymbol: name.Name, kind: kind, node: "ValueSpec",
									start: item.Pos(), end: item.End(),
									nameStart: name.Pos(), nameEnd: name.End(),
								})
							}
						}
					}
				}
			}
		}
	}

	rows := make([]unitRecord, 0, len(m.Units))
	seen := map[string]struct{}{}
	for _, unit := range m.Units {
		key := unit.Path + "\x00" + unit.Symbol
		if _, exists := seen[key]; exists {
			return fmt.Errorf("duplicate manifest unit %s", unit.ID)
		}
		seen[key] = struct{}{}
		matches := make([]astDeclaration, 0, 1)
		for _, candidate := range declarations[unit.Path] {
			if candidate.unitSymbol == unit.Symbol {
				matches = append(matches, candidate)
			}
		}
		if len(matches) != 1 {
			return fmt.Errorf("%s: expected one Go AST declaration for %q, found %d", unit.Path, unit.Symbol, len(matches))
		}
		match := matches[0]
		start, end := fileSet.Position(match.start), fileSet.Position(match.end)
		rows = append(rows, unitRecord{
			ID: unit.ID, Path: unit.Path, Symbol: unit.Symbol, Kind: match.kind, ASTNode: match.node,
			StartByte:           fileSet.PositionFor(match.start, false).Offset,
			EndByte:             fileSet.PositionFor(match.end, false).Offset,
			IdentifierStartByte: fileSet.PositionFor(match.nameStart, false).Offset,
			IdentifierEndByte:   fileSet.PositionFor(match.nameEnd, false).Offset,
			StartLine:           start.Line,
			EndLine:             end.Line,
		})
	}
	sort.Slice(rows, func(i, j int) bool { return rows[i].ID < rows[j].ID })
	result := output{
		Schema: "go-source-unit-inventory-v1", FixtureID: m.FixtureID, Language: m.Language,
		Parser:       runtime.Version(),
		Method:       "go/parser AST declaration and identifier spans; independent of SCIP and tree-sitter",
		SourceSHA256: hex.EncodeToString(selectedDigest.Sum(nil)), SourceFiles: len(orderedPaths),
		SourceBytes: totalBytes, Units: rows,
	}
	encoded, err := json.MarshalIndent(result, "", "  ")
	if err != nil {
		return err
	}
	encoded = append(encoded, '\n')
	if outputPath == "" {
		_, err = os.Stdout.Write(encoded)
		return err
	}
	return os.WriteFile(outputPath, encoded, 0o644)
}

func receiverName(expression ast.Expr) string {
	switch node := expression.(type) {
	case *ast.Ident:
		return node.Name
	case *ast.StarExpr:
		return receiverName(node.X)
	case *ast.IndexExpr:
		return receiverName(node.X)
	case *ast.IndexListExpr:
		return receiverName(node.X)
	case *ast.SelectorExpr:
		return receiverName(node.Sel)
	default:
		return ""
	}
}

func containsDotDot(path string) bool {
	for _, part := range strings.Split(filepath.ToSlash(path), "/") {
		if part == ".." || part == "." || part == "" {
			return true
		}
	}
	return false
}

func fatalf(format string, args ...any) {
	fmt.Fprintf(os.Stderr, format+"\n", args...)
	os.Exit(2)
}
