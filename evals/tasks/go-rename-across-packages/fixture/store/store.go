// Package store keeps inventory items in memory.
package store

import (
	"context"
	"errors"
	"sort"
	"sync"
)

// Item is one stock-keeping unit.
type Item struct {
	SKU        string `json:"sku"`
	Name       string `json:"name"`
	Quantity   int    `json:"quantity"`
	PriceCents int64  `json:"price_cents"`
}

var (
	// ItemNotFoundErr is returned by GetItem and Adjust when no item has
	// the requested SKU.
	ItemNotFoundErr = errors.New("store: item not found")
	// ErrNegativeStock is returned by Adjust when stock would drop below zero.
	ErrNegativeStock = errors.New("store: stock cannot go negative")
)

// Store is a concurrency-safe in-memory inventory.
type Store struct {
	mu    sync.RWMutex
	items map[string]Item
}

// New returns a store holding items.
func New(items ...Item) *Store {
	s := &Store{items: make(map[string]Item, len(items))}
	for _, item := range items {
		s.items[item.SKU] = item
	}
	return s
}

// GetItem returns the item with the given SKU, or ItemNotFoundErr.
func (s *Store) GetItem(ctx context.Context, sku string) (Item, error) {
	if err := ctx.Err(); err != nil {
		return Item{}, err
	}
	s.mu.RLock()
	defer s.mu.RUnlock()
	item, ok := s.items[sku]
	if !ok {
		return Item{}, ItemNotFoundErr
	}
	return item, nil
}

// GetItems returns every item, sorted by SKU.
func (s *Store) GetItems(ctx context.Context) ([]Item, error) {
	if err := ctx.Err(); err != nil {
		return nil, err
	}
	s.mu.RLock()
	defer s.mu.RUnlock()
	items := make([]Item, 0, len(s.items))
	for _, item := range s.items {
		items = append(items, item)
	}
	sort.Slice(items, func(i, j int) bool { return items[i].SKU < items[j].SKU })
	return items, nil
}

// GetItemCount returns the number of distinct SKUs.
func (s *Store) GetItemCount() int {
	s.mu.RLock()
	defer s.mu.RUnlock()
	return len(s.items)
}

// Put adds or replaces an item.
func (s *Store) Put(item Item) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.items[item.SKU] = item
}

// Adjust changes an item's quantity by delta and returns the updated item.
// It fails with ItemNotFoundErr for an unknown SKU and ErrNegativeStock if
// the quantity would drop below zero.
func (s *Store) Adjust(ctx context.Context, sku string, delta int) (Item, error) {
	if err := ctx.Err(); err != nil {
		return Item{}, err
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	item, ok := s.items[sku]
	if !ok {
		return Item{}, ItemNotFoundErr
	}
	if item.Quantity+delta < 0 {
		return Item{}, ErrNegativeStock
	}
	item.Quantity += delta
	s.items[sku] = item
	return item, nil
}
