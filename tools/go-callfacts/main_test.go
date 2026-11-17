package main

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"go/ast"
	"go/parser"
	"go/token"
	"os"
	"path/filepath"
	"reflect"
	"runtime"
	"sort"
	"strings"
	"testing"
)

type frozenTruth struct {
	SourceSHA256 map[string]string `json:"source_sha256"`
	Calls        []CallFact        `json:"calls"`
}

func TestGoTypesFactsMatchFrozenCallTruth(t *testing.T) {
	root := fixtureRoot(t)
	truthBytes, err := os.ReadFile(filepath.Join(root, "truth.json"))
	if err != nil {
		t.Fatal(err)
	}
	var truth frozenTruth
	if err := json.Unmarshal(truthBytes, &truth); err != nil {
		t.Fatal(err)
	}
	for relative, expectedDigest := range truth.SourceSHA256 {
		contents, err := os.ReadFile(filepath.Join(root, relative))
		if err != nil {
			t.Fatal(err)
		}
		digest := sha256.Sum256(contents)
		if got := hex.EncodeToString(digest[:]); got != expectedDigest {
			t.Errorf("source digest for %s: got %s, want %s", relative, got, expectedDigest)
		}
	}

	artifact, err := buildCallFacts(root)
	if err != nil {
		t.Fatal(err)
	}
	if artifact.Schema != callFactsSchema || artifact.Version != callFactsVersion {
		t.Fatalf("unexpected sidecar identity: %s v%d", artifact.Schema, artifact.Version)
	}
	gotFiles := make(map[string]string, len(artifact.Files))
	for _, file := range artifact.Files {
		gotFiles[file.Path] = file.SHA256
	}
	for path, digest := range truth.SourceSHA256 {
		if path == "go.mod" {
			continue
		}
		if gotFiles[path] != digest {
			t.Errorf("sidecar source digest for %s: got %s, want %s", path, gotFiles[path], digest)
		}
	}
	if !hasContextFile(artifact.Context.ContextFiles, "go.mod") {
		t.Fatalf("Go context did not attest fixture go.mod: %#v", artifact.Context.ContextFiles)
	}
	if got := goAnalysisContextSHA256(artifact.Context); got != artifact.ContextSHA256 {
		t.Fatalf("Go context fingerprint: got %s, want %s", artifact.ContextSHA256, got)
	}

	if !reflect.DeepEqual(artifact.Calls, truth.Calls) {
		got, _ := json.MarshalIndent(artifact.Calls, "", "  ")
		want, _ := json.MarshalIndent(truth.Calls, "", "  ")
		t.Fatalf("call facts differ from frozen source truth:\n got: %s\nwant: %s", got, want)
	}
}

func TestCallFactsWriterReplacesSidecarAtomically(t *testing.T) {
	directory := t.TempDir()
	path := filepath.Join(directory, ".zvec-grep", callFactsFile)
	want := CallFactsArtifact{
		Schema: callFactsSchema, Version: callFactsVersion,
		Context: GoAnalysisContext{
			Settings:     map[string]string{},
			ContextFiles: []SourceFile{},
		},
		Files: []SourceFile{}, Calls: []CallFact{},
	}
	if err := writeArtifactAtomically(path, want); err != nil {
		t.Fatal(err)
	}
	contents, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	var got CallFactsArtifact
	if err := json.Unmarshal(contents, &got); err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("sidecar round-trip: got %#v, want %#v", got, want)
	}
	entries, err := os.ReadDir(filepath.Dir(path))
	if err != nil {
		t.Fatal(err)
	}
	if len(entries) != 1 || entries[0].Name() != callFactsFile {
		t.Fatalf("temporary file was not cleaned up: %#v", entries)
	}
}

func TestTypeCheckFailureLeavesExistingSidecarUntouched(t *testing.T) {
	root := t.TempDir()
	if err := os.WriteFile(filepath.Join(root, "go.mod"), []byte("module example.com/invalid\n\ngo 1.22\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(root, "main.go"), []byte("package invalid\nfunc caller() { missing() }\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	path := filepath.Join(root, ".zvec-grep", callFactsFile)
	previous := []byte("previous valid sidecar")
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, previous, 0o600); err != nil {
		t.Fatal(err)
	}
	if _, err := buildCallFacts(root); err == nil {
		t.Fatal("type-check failure should prevent facts generation")
	}
	got, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if string(got) != string(previous) {
		t.Fatalf("existing sidecar changed on analysis failure: %q", got)
	}
}

func TestInactiveGoFilesAreEmittedAsUnresolved(t *testing.T) {
	root := t.TempDir()
	if err := os.WriteFile(filepath.Join(root, "go.mod"), []byte("module example.com/inactive\n\ngo 1.22\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	active := []byte("package inactive\nfunc target() {}\nfunc active() { target() }\n")
	inactive := []byte("//go:build never\n\npackage inactive\nfunc inactive() { missing() }\n")
	if err := os.WriteFile(filepath.Join(root, "active.go"), active, 0o600); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(root, "inactive.go"), inactive, 0o600); err != nil {
		t.Fatal(err)
	}
	artifact, err := buildCallFacts(root)
	if err != nil {
		t.Fatal(err)
	}
	var inactiveFact *CallFact
	for index := range artifact.Calls {
		if artifact.Calls[index].Path == "inactive.go" {
			inactiveFact = &artifact.Calls[index]
		}
	}
	if inactiveFact == nil || inactiveFact.Resolution != "unresolved" || inactiveFact.Target != nil {
		t.Fatalf("inactive build-tag call must remain unresolved: %#v", inactiveFact)
	}
}

func TestGoContextRejectsExternalLocalReplacement(t *testing.T) {
	base := t.TempDir()
	root := filepath.Join(base, "root")
	outside := filepath.Join(base, "external")
	if err := os.MkdirAll(root, 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.MkdirAll(outside, 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(outside, "go.mod"), []byte("module example.com/external\n\ngo 1.22\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(root, "go.mod"), []byte("module example.com/root\n\ngo 1.22\n\nreplace example.com/external => ../"+filepath.Base(outside)+"\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(root, "main.go"), []byte("package root\nfunc caller() {}\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	t.Setenv("GOENV", "off")
	t.Setenv("GOWORK", "off")
	t.Setenv("GOPACKAGESDRIVER", "")
	if _, err := buildCallFacts(root); err == nil || !strings.Contains(err.Error(), "outside the Go workspace root") {
		t.Fatalf("external local replacement should be rejected, got %v", err)
	}
}

func TestGoContextFingerprintIncludesBuildConfiguration(t *testing.T) {
	root := fixtureRoot(t)
	t.Setenv("GOENV", "off")
	t.Setenv("GOWORK", "off")
	t.Setenv("GOFLAGS", "-tags=callfacts_context_test")
	context, err := captureGoAnalysisContext(root, true)
	if err != nil {
		t.Fatal(err)
	}
	if got := context.Settings["GOFLAGS"]; got != "-tags=callfacts_context_test" {
		t.Fatalf("captured GOFLAGS %q", got)
	}
	first := goAnalysisContextSHA256(context)
	context.Settings["GOFLAGS"] = "-tags=another_context"
	if second := goAnalysisContextSHA256(context); first == second {
		t.Fatal("context fingerprint did not change with GOFLAGS")
	}
}

func TestGoContextFingerprintUsesSharedCrossLanguageContract(t *testing.T) {
	settings := make(map[string]string, len(goContextSettingNames))
	for _, name := range goContextSettingNames {
		settings[name] = ""
	}
	settings["GOARCH"] = "arm64"
	settings["CGO_ENABLED"] = "1"
	settings["GOFLAGS"] = "-tags=fixture"
	settings["GOOS"] = "darwin"
	context := GoAnalysisContext{
		GoVersion: "go1.26.0",
		GoMod:     "go.mod",
		Settings:  settings,
		ContextFiles: []SourceFile{{
			Path:   "go.mod",
			SHA256: "abc123",
		}},
	}
	const want = "5b7f099e8fbdb5261a1e04b1a2183cbecc299f34b7f13a2583f7d0e9f48e0505"
	if got := goAnalysisContextSHA256(context); got != want {
		t.Fatalf("cross-language context fingerprint: got %s, want %s", got, want)
	}
}

func TestGoContextInputsIncludeVendorManifest(t *testing.T) {
	root := t.TempDir()
	for path, contents := range map[string]string{
		"go.mod":                 "module example.com/vendor\n\ngo 1.22\n",
		"vendor/modules.txt":     "# example.com/dep v1.0.0\nexample.com/dep\n",
		"not-vendor/modules.txt": "not a Go vendor manifest\n",
	} {
		fullPath := filepath.Join(root, filepath.FromSlash(path))
		if err := os.MkdirAll(filepath.Dir(fullPath), 0o700); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(fullPath, []byte(contents), 0o600); err != nil {
			t.Fatal(err)
		}
	}
	files, err := collectGoContextFiles(root)
	if err != nil {
		t.Fatal(err)
	}
	if !hasContextFile(files, "vendor/modules.txt") {
		t.Fatalf("vendor/modules.txt missing from analysis inputs: %#v", files)
	}
	if hasContextFile(files, "not-vendor/modules.txt") {
		t.Fatalf("non-vendor modules.txt unexpectedly included: %#v", files)
	}
}

func TestGoContextRejectsUnpinnedGOFLAGSInputs(t *testing.T) {
	for _, flags := range []string{
		"-overlay=/tmp/overlay.json",
		"-overlay /tmp/overlay.json",
		"-modfile=/tmp/alternate.mod",
		"-toolexec=/tmp/wrapper",
		"-pkgdir /tmp/pkgdir",
	} {
		if err := validateGoFlags(flags); err == nil {
			t.Errorf("GOFLAGS %q should be rejected", flags)
		}
	}
	if err := validateGoFlags(`-tags="context matrix" -mod=vendor`); err != nil {
		t.Fatalf("supported build flags unexpectedly rejected: %v", err)
	}
}

func TestPackageUsesCgoDetectsCImports(t *testing.T) {
	file, err := parser.ParseFile(token.NewFileSet(), "cgo.go", "package sample\nimport \"C\"\n", 0)
	if err != nil {
		t.Fatal(err)
	}
	if !packageUsesCgo([]*ast.File{file}) {
		t.Fatal("import C was not recognized")
	}
	noncgo, err := parser.ParseFile(token.NewFileSet(), "plain.go", "package sample\nimport \"fmt\"\n", 0)
	if err != nil {
		t.Fatal(err)
	}
	if packageUsesCgo([]*ast.File{noncgo}) {
		t.Fatal("ordinary import was misclassified as cgo")
	}
}

func fixtureRoot(t *testing.T) string {
	t.Helper()
	_, sourceFile, _, ok := runtime.Caller(0)
	if !ok {
		t.Fatal("locate test source")
	}
	projectRoot, err := filepath.Abs(filepath.Join(filepath.Dir(sourceFile), "..", ".."))
	if err != nil {
		t.Fatal(err)
	}
	return filepath.Join(projectRoot, "rust", "crates", "zg-codegraph", "tests", "fixtures", "go-blast-radius")
}

func TestCallFactsAreStableOrdered(t *testing.T) {
	root := fixtureRoot(t)
	first, err := buildCallFacts(root)
	if err != nil {
		t.Fatal(err)
	}
	second, err := buildCallFacts(root)
	if err != nil {
		t.Fatal(err)
	}
	if !sort.SliceIsSorted(first.Calls, func(i, j int) bool {
		return first.Calls[i].Path < first.Calls[j].Path ||
			(first.Calls[i].Path == first.Calls[j].Path && first.Calls[i].StartByte < first.Calls[j].StartByte)
	}) {
		t.Fatal("call facts are not source ordered")
	}
	if !reflect.DeepEqual(first, second) {
		t.Fatal("call facts are not deterministic")
	}
}
