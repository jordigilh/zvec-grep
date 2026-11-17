package main

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"os"
	"path/filepath"
	"runtime"
	"testing"
)

type contextMatrixTruth struct {
	SourceSHA256 map[string]string `json:"source_sha256"`
	Profiles     map[string]struct {
		GOOS           string `json:"goos"`
		GOFLAGS        string `json:"goflags"`
		PlatformTarget string `json:"platform_target"`
		TaggedTarget   string `json:"tagged_target"`
	} `json:"profiles"`
	GenericTarget string `json:"generic_target"`
}

func TestContextMatrixCallFactsMatchFrozenTruth(t *testing.T) {
	root := contextMatrixFixtureRoot(t)
	truthBytes, err := os.ReadFile(filepath.Join(root, "truth.json"))
	if err != nil {
		t.Fatal(err)
	}
	var truth contextMatrixTruth
	if err := json.Unmarshal(truthBytes, &truth); err != nil {
		t.Fatal(err)
	}
	for relative, expected := range truth.SourceSHA256 {
		contents, err := os.ReadFile(filepath.Join(root, relative))
		if err != nil {
			t.Fatal(err)
		}
		if got := sha256Hex(contents); got != expected {
			t.Errorf("context matrix source digest for %s: got %s, want %s", relative, got, expected)
		}
	}

	t.Setenv("GOENV", "off")
	t.Setenv("GOWORK", "off")
	t.Setenv("GOPACKAGESDRIVER", "")
	t.Setenv("GOARCH", "amd64")
	t.Setenv("CGO_ENABLED", "0")
	var firstContext string
	for _, profileName := range []string{"linux-default", "windows-special"} {
		profile := truth.Profiles[profileName]
		t.Setenv("GOOS", profile.GOOS)
		t.Setenv("GOFLAGS", profile.GOFLAGS)
		artifact, err := buildCallFacts(root)
		if err != nil {
			t.Fatalf("profile %s: %v", profileName, err)
		}
		if artifact.Context.Settings["GOOS"] != profile.GOOS || artifact.Context.Settings["GOFLAGS"] != profile.GOFLAGS {
			t.Errorf("profile %s context settings = GOOS %q GOFLAGS %q", profileName, artifact.Context.Settings["GOOS"], artifact.Context.Settings["GOFLAGS"])
		}
		if firstContext == "" {
			firstContext = artifact.ContextSHA256
		} else if artifact.ContextSHA256 == firstContext {
			t.Error("different Go build profiles shared a context fingerprint")
		}

		assertStaticTarget(t, artifact.Calls, "matrix/caller.go::matrix.platformCaller", profile.PlatformTarget)
		assertStaticTarget(t, artifact.Calls, "matrix/caller.go::matrix.taggedCaller", profile.TaggedTarget)
		assertStaticTarget(t, artifact.Calls, "matrix/caller.go::matrix.genericCaller", truth.GenericTarget)
		assertExternal(t, artifact.Calls, "matrix/caller.go::matrix.externalCaller")

		inactiveCaller := "matrix/platform_windows.go::matrix.platformOnlyCaller"
		if profile.GOOS == "windows" {
			inactiveCaller = "matrix/platform_linux.go::matrix.platformOnlyCaller"
		}
		assertUnresolved(t, artifact.Calls, inactiveCaller)
	}
}

func assertStaticTarget(t *testing.T, facts []CallFact, caller, target string) {
	t.Helper()
	for _, fact := range facts {
		if fact.Caller == caller {
			if fact.Resolution != "static" || fact.Target == nil || *fact.Target != target {
				t.Fatalf("call fact for %s = (%s, %v), want static target %s", caller, fact.Resolution, fact.Target, target)
			}
			return
		}
	}
	t.Fatalf("missing call fact for %s", caller)
}

func assertUnresolved(t *testing.T, facts []CallFact, caller string) {
	t.Helper()
	for _, fact := range facts {
		if fact.Caller == caller {
			if fact.Resolution != "unresolved" || fact.Target != nil {
				t.Fatalf("call fact for %s = (%s, %v), want unresolved", caller, fact.Resolution, fact.Target)
			}
			return
		}
	}
	t.Fatalf("missing call fact for %s", caller)
}

func assertExternal(t *testing.T, facts []CallFact, caller string) {
	t.Helper()
	for _, fact := range facts {
		if fact.Caller == caller {
			if fact.Resolution != "external" || fact.Target != nil {
				t.Fatalf("call fact for %s = (%s, %v), want external", caller, fact.Resolution, fact.Target)
			}
			return
		}
	}
	t.Fatalf("missing call fact for %s", caller)
}

func contextMatrixFixtureRoot(t *testing.T) string {
	t.Helper()
	_, sourceFile, _, ok := runtime.Caller(0)
	if !ok {
		t.Fatal("locate test source")
	}
	projectRoot, err := filepath.Abs(filepath.Join(filepath.Dir(sourceFile), "..", ".."))
	if err != nil {
		t.Fatal(err)
	}
	return filepath.Join(projectRoot, "rust", "crates", "zg-codegraph", "tests", "fixtures", "go-context-matrix")
}

func sha256Hex(contents []byte) string {
	digest := sha256.Sum256(contents)
	return hex.EncodeToString(digest[:])
}
