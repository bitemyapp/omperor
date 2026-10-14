package cache

import (
	"context"
	"errors"
	"testing"
	"time"

	"example.com/stockroom/store"
)

// countingGetter records how often FetchItem reaches the backing source.
type countingGetter struct {
	items map[string]store.Item
	calls int
}

func (g *countingGetter) FetchItem(_ context.Context, sku string) (store.Item, error) {
	g.calls++
	item, ok := g.items[sku]
	if !ok {
		return store.Item{}, store.ErrItemNotFound
	}
	return item, nil
}

func (g *countingGetter) GetItems(context.Context) ([]store.Item, error) {
	return nil, nil
}

func TestCachesHitsAndMisses(t *testing.T) {
	inner := &countingGetter{items: map[string]store.Item{"A": {SKU: "A", Name: "Anvil"}}}
	c := New(inner, time.Minute)
	ctx := context.Background()
	for i := 0; i < 3; i++ {
		if item, err := c.FetchItem(ctx, "A"); err != nil || item.Name != "Anvil" {
			t.Fatalf("FetchItem(A) = %+v, %v", item, err)
		}
		if _, err := c.FetchItem(ctx, "missing"); !errors.Is(err, store.ErrItemNotFound) {
			t.Fatalf("FetchItem(missing) = %v", err)
		}
	}
	if inner.calls != 2 {
		t.Fatalf("inner FetchItem called %d times, want 2", inner.calls)
	}
}

func TestEntriesExpire(t *testing.T) {
	inner := &countingGetter{items: map[string]store.Item{"A": {SKU: "A"}}}
	c := New(inner, time.Second)
	now := time.Unix(1000, 0)
	c.now = func() time.Time { return now }
	ctx := context.Background()
	c.FetchItem(ctx, "A")
	now = now.Add(2 * time.Second)
	c.FetchItem(ctx, "A")
	if inner.calls != 2 {
		t.Fatalf("inner FetchItem called %d times, want 2", inner.calls)
	}
}
