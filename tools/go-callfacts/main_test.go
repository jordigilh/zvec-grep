package main

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"os"
	"path/filepath"
	"reflect"
	"runtime"
	"sort"
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
