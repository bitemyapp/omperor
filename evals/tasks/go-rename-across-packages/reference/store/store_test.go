package store

import (
	"context"
	"errors"
	"testing"
)

func TestGetItem(t *testing.T) {
	s := New(Item{SKU: "A", Name: "Anvil", Quantity: 2})
	item, err := s.FetchItem(context.Background(), "A")
	if err != nil || item.Name != "Anvil" {
		t.Fatalf("FetchItem(A) = %+v, %v", item, err)
	}
	if _, err := s.FetchItem(context.Background(), "B"); !errors.Is(err, ErrItemNotFound) {
		t.Fatalf("FetchItem(B) error = %v, want ErrItemNotFound", err)
	}
}

func TestGetItemsSortedAndCount(t *testing.T) {
	s := New(Item{SKU: "B"}, Item{SKU: "A"}, Item{SKU: "C"})
	items, err := s.GetItems(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if len(items) != 3 || items[0].SKU != "A" || items[2].SKU != "C" {
		t.Fatalf("GetItems = %+v", items)
	}
	if s.GetItemCount() != 3 {
		t.Fatalf("GetItemCount = %d", s.GetItemCount())
	}
}

func TestAdjust(t *testing.T) {
	s := New(Item{SKU: "A", Quantity: 1})
	ctx := context.Background()
	if _, err := s.Adjust(ctx, "A", -2); !errors.Is(err, ErrNegativeStock) {
		t.Fatalf("Adjust below zero error = %v", err)
	}
	if _, err := s.Adjust(ctx, "Z", 1); !errors.Is(err, ErrItemNotFound) {
		t.Fatalf("Adjust unknown error = %v", err)
	}
	item, err := s.Adjust(ctx, "A", 4)
	if err != nil || item.Quantity != 5 {
		t.Fatalf("Adjust = %+v, %v", item, err)
	}
}
