// Package report builds the restocking reports printed each morning.
package report

import (
	"context"
	"errors"
	"fmt"
	"io"

	"example.com/stockroom/store"
)

// ItemSource looks up single items; *store.Store and *cache.Cached both work.
type ItemSource interface {
	GetItem(ctx context.Context, sku string) (store.Item, error)
}

// LowStock looks up each SKU with GetItem and returns, in the order given,
// the items whose quantity is at or below threshold. SKUs that are not in the
// store (store.ItemNotFoundErr) are skipped; any other error stops the report.
func LowStock(ctx context.Context, source ItemSource, skus []string, threshold int) ([]store.Item, error) {
	var low []store.Item
	for _, sku := range skus {
		item, err := source.GetItem(ctx, sku)
		if errors.Is(err, store.ItemNotFoundErr) {
			continue
		}
		if err != nil {
			return nil, fmt.Errorf("report: %s: %w", sku, err)
		}
		if item.Quantity <= threshold {
			low = append(low, item)
		}
	}
	return low, nil
}

// Write prints one line per item.
func Write(w io.Writer, items []store.Item) error {
	for _, item := range items {
		if _, err := fmt.Fprintf(w, "%-12s %-28s %4d\n", item.SKU, item.Name, item.Quantity); err != nil {
			return err
		}
	}
	return nil
}
