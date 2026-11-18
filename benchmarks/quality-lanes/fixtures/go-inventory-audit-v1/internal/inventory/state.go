package inventory

import (
	"context"
	"sync"
)

// AssetState records asset IDs returned by list_assets for one selection
// context. Rechecks and later scan calls share this object.
type AssetState struct {
	mu  sync.RWMutex
	ids map[string]struct{}
}

func NewAssetState() *AssetState {
	return &AssetState{ids: make(map[string]struct{})}
}

// Add records every non-empty asset ID returned by the scanner.
func (s *AssetState) Add(assetIDs ...string) {
	if s == nil {
		return
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.ids == nil {
		s.ids = make(map[string]struct{})
	}
	for _, id := range assetIDs {
		if id != "" {
			s.ids[id] = struct{}{}
		}
	}
}

func (s *AssetState) Contains(assetID string) bool {
	if s == nil {
		return false
	}
	s.mu.RLock()
	defer s.mu.RUnlock()
	_, ok := s.ids[assetID]
	return ok
}

type assetStateKey struct{}

func WithAssetState(ctx context.Context, state *AssetState) context.Context {
	return context.WithValue(ctx, assetStateKey{}, state)
}

func AssetStateFromContext(ctx context.Context) (*AssetState, bool) {
	state, ok := ctx.Value(assetStateKey{}).(*AssetState)
	return state, ok && state != nil
}
