package report

import (
	"bytes"
	"context"
	"strings"
	"testing"

	"example.com/stockroom/store"
)

func TestLowStockSkipsUnknownSKUs(t *testing.T) {
	s := store.New(
		store.Item{SKU: "A", Name: "Anvil", Quantity: 1},
		store.Item{SKU: "B", Name: "Bolt", Quantity: 900},
	)
	low, err := LowStock(context.Background(), s, []string{"A", "B", "nope"}, 5)
	if err != nil {
		t.Fatal(err)
	}
	if len(low) != 1 || low[0].SKU != "A" {
		t.Fatalf("LowStock = %+v", low)
	}
	var out bytes.Buffer
	if err := Write(&out, low); err != nil || !strings.Contains(out.String(), "Anvil") {
		t.Fatalf("Write = %q, %v", out.String(), err)
	}
}
