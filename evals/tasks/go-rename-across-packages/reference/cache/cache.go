// Package cache adds a read-through, time-limited cache in front of any item
// source, such as a *store.Store.
package cache

import (
	"context"
	"errors"
	"sync"
	"time"

	"example.com/stockroom/store"
)

// Getter is the read side the cache wraps.
type Getter interface {
	FetchItem(ctx context.Context, sku string) (store.Item, error)
	GetItems(ctx context.Context) ([]store.Item, error)
}

type entry struct {
	item    store.Item
	missing bool
	expires time.Time
}

// Cached serves FetchItem from memory for up to ttl, including misses.
type Cached struct {
	inner Getter
	ttl   time.Duration
	now   func() time.Time

	mu      sync.Mutex
	entries map[string]entry
}

// New wraps inner with a cache whose entries live for ttl.
func New(inner Getter, ttl time.Duration) *Cached {
	return &Cached{inner: inner, ttl: ttl, now: time.Now, entries: map[string]entry{}}
}

// FetchItem returns the cached item, or asks the wrapped Getter. A
// store.ErrItemNotFound answer is cached too, so repeated scans of an unknown
// label do not hit the store.
func (c *Cached) FetchItem(ctx context.Context, sku string) (store.Item, error) {
	c.mu.Lock()
	cached, ok := c.entries[sku]
	c.mu.Unlock()
	if ok && c.now().Before(cached.expires) {
		if cached.missing {
			return store.Item{}, store.ErrItemNotFound
		}
		return cached.item, nil
	}
	item, err := c.inner.FetchItem(ctx, sku)
	switch {
	case errors.Is(err, store.ErrItemNotFound):
		c.remember(sku, entry{missing: true})
	case err == nil:
		c.remember(sku, entry{item: item})
	}
	return item, err
}

// GetItems is not cached: listings are rare and must be fresh.
func (c *Cached) GetItems(ctx context.Context) ([]store.Item, error) {
	return c.inner.GetItems(ctx)
}

// Forget drops any cached answer for sku.
func (c *Cached) Forget(sku string) {
	c.mu.Lock()
	defer c.mu.Unlock()
	delete(c.entries, sku)
}

func (c *Cached) remember(sku string, e entry) {
	e.expires = c.now().Add(c.ttl)
	c.mu.Lock()
	defer c.mu.Unlock()
	c.entries[sku] = e
}
