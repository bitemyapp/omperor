package api

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"example.com/stockroom/store"
)

// fakeItems is a fixed ItemGetter for handler tests.
type fakeItems map[string]store.Item

func (f fakeItems) FetchItem(_ context.Context, sku string) (store.Item, error) {
	item, ok := f[sku]
	if !ok {
		return store.Item{}, store.ErrItemNotFound
	}
	return item, nil
}

func (f fakeItems) GetItems(context.Context) ([]store.Item, error) {
	items := make([]store.Item, 0, len(f))
	for _, item := range f {
		items = append(items, item)
	}
	return items, nil
}

func post(t *testing.T, s *Server, body string) map[string]any {
	t.Helper()
	recorder := httptest.NewRecorder()
	s.ServeHTTP(recorder, httptest.NewRequest(http.MethodPost, "/rpc", strings.NewReader(body)))
	var decoded map[string]any
	if err := json.Unmarshal(recorder.Body.Bytes(), &decoded); err != nil {
		t.Fatalf("response %q: %v", recorder.Body.String(), err)
	}
	return decoded
}

func TestGetItemOverRPC(t *testing.T) {
	s := NewServer(fakeItems{"A": {SKU: "A", Name: "Anvil", Quantity: 3}})
	got := post(t, s, `{"method":"Inventory.GetItem","params":{"sku":"A"}}`)
	result, _ := got["result"].(map[string]any)
	if result["name"] != "Anvil" {
		t.Fatalf("result = %v", got)
	}
	got = post(t, s, `{"method":"Inventory.GetItem","params":{"sku":"Z"}}`)
	errObj, _ := got["error"].(map[string]any)
	if errObj["code"] != float64(404) {
		t.Fatalf("missing SKU = %v", got)
	}
}

func TestCount(t *testing.T) {
	s := NewServer(fakeItems{"A": {SKU: "A"}, "B": {SKU: "B"}})
	got := post(t, s, `{"method":"Inventory.Count"}`)
	result, _ := got["result"].(map[string]any)
	if result["count"] != float64(2) {
		t.Fatalf("count = %v", got)
	}
}
