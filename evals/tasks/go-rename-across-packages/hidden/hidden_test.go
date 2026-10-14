// Hidden grader test: copied into the module as zz_hidden/hidden_test.go.
package hidden_test

import (
	"context"
	"encoding/json"
	"errors"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"example.com/stockroom/api"
	"example.com/stockroom/cache"
	"example.com/stockroom/report"
	"example.com/stockroom/store"
)

var (
	_ api.ItemGetter    = (*store.Store)(nil)
	_ api.ItemGetter    = (*cache.Cached)(nil)
	_ cache.Getter      = (*store.Store)(nil)
	_ report.ItemSource = (*store.Store)(nil)
	_ report.ItemSource = (*cache.Cached)(nil)
)

func demo() *store.Store {
	return store.New(
		store.Item{SKU: "A-1", Name: "Anvil", Quantity: 3, PriceCents: 4999},
		store.Item{SKU: "B-2", Name: "Bolt", Quantity: 50, PriceCents: 12},
	)
}

func TestHiddenFetchItem(t *testing.T) {
	ctx := context.Background()
	s := demo()
	item, err := s.FetchItem(ctx, "A-1")
	if err != nil || item.Name != "Anvil" || item.PriceCents != 4999 {
		t.Fatalf("FetchItem(A-1) = %+v, %v", item, err)
	}
	if _, err := s.FetchItem(ctx, "nope"); !errors.Is(err, store.ErrItemNotFound) {
		t.Fatalf("FetchItem(nope) error = %v", err)
	}
	if _, err := s.Adjust(ctx, "nope", 1); !errors.Is(err, store.ErrItemNotFound) {
		t.Fatalf("Adjust(nope) error = %v", err)
	}
	if store.ErrItemNotFound.Error() != "store: item not found" {
		t.Fatalf("ErrItemNotFound message changed: %q", store.ErrItemNotFound)
	}
	if n := s.GetItemCount(); n != 2 {
		t.Fatalf("GetItemCount = %d", n)
	}
	if items, err := s.GetItems(ctx); err != nil || len(items) != 2 || items[0].SKU != "A-1" {
		t.Fatalf("GetItems = %+v, %v", items, err)
	}
}

type countingSource struct {
	inner *store.Store
	calls int
}

func (c *countingSource) FetchItem(ctx context.Context, sku string) (store.Item, error) {
	c.calls++
	return c.inner.FetchItem(ctx, sku)
}

func (c *countingSource) GetItems(ctx context.Context) ([]store.Item, error) {
	return c.inner.GetItems(ctx)
}

func TestHiddenCacheFetchItem(t *testing.T) {
	ctx := context.Background()
	source := &countingSource{inner: demo()}
	c := cache.New(source, time.Minute)
	for i := 0; i < 3; i++ {
		if item, err := c.FetchItem(ctx, "A-1"); err != nil || item.Name != "Anvil" {
			t.Fatalf("FetchItem(A-1) = %+v, %v", item, err)
		}
		if _, err := c.FetchItem(ctx, "nope"); !errors.Is(err, store.ErrItemNotFound) {
			t.Fatalf("FetchItem(nope) error = %v", err)
		}
	}
	if source.calls != 2 {
		t.Fatalf("source called %d times, want 2 (hit and miss both cached)", source.calls)
	}
}

func rpc(t *testing.T, handler http.Handler, body string) map[string]any {
	t.Helper()
	recorder := httptest.NewRecorder()
	handler.ServeHTTP(recorder, httptest.NewRequest(http.MethodPost, "/rpc", strings.NewReader(body)))
	var decoded map[string]any
	if err := json.Unmarshal(recorder.Body.Bytes(), &decoded); err != nil {
		t.Fatalf("response %q: %v", recorder.Body.String(), err)
	}
	return decoded
}

func TestHiddenWireProtocolUnchanged(t *testing.T) {
	server := api.NewServer(cache.New(demo(), time.Minute))
	got := rpc(t, server, `{"method":"Inventory.GetItem","params":{"sku":"A-1"}}`)
	result, _ := got["result"].(map[string]any)
	if result["name"] != "Anvil" || result["sku"] != "A-1" {
		t.Fatalf("Inventory.GetItem = %v", got)
	}
	got = rpc(t, server, `{"method":"Inventory.GetItem","params":{"sku":"nope"}}`)
	failure, _ := got["error"].(map[string]any)
	if failure["code"] != float64(404) || failure["message"] != "item not found" {
		t.Fatalf("Inventory.GetItem(nope) = %v", got)
	}
	got = rpc(t, server, `{"method":"Inventory.Count"}`)
	result, _ = got["result"].(map[string]any)
	if result["count"] != float64(2) {
		t.Fatalf("Inventory.Count = %v", got)
	}
}

func TestHiddenReport(t *testing.T) {
	low, err := report.LowStock(context.Background(), demo(), []string{"B-2", "nope", "A-1"}, 5)
	if err != nil || len(low) != 1 || low[0].SKU != "A-1" {
		t.Fatalf("LowStock = %+v, %v", low, err)
	}
}
