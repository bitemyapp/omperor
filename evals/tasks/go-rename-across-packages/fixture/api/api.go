// Package api serves the JSON-RPC endpoint used by the handheld scanners.
//
// The method names below are a public contract with scanner firmware and must
// never be renamed, whatever the Go API is called.
package api

import (
	"context"
	"encoding/json"
	"errors"
	"net/http"

	"example.com/stockroom/store"
)

// Wire method names.
const (
	MethodGetItem   = "Inventory.GetItem"
	MethodListItems = "Inventory.ListItems"
	MethodCount     = "Inventory.Count"
)

// ItemGetter is what the server reads from: a *store.Store or a *cache.Cached.
type ItemGetter interface {
	GetItem(ctx context.Context, sku string) (store.Item, error)
	GetItems(ctx context.Context) ([]store.Item, error)
}

// Server answers JSON-RPC requests.
type Server struct {
	items ItemGetter
}

// NewServer returns a server reading from items.
func NewServer(items ItemGetter) *Server {
	return &Server{items: items}
}

type request struct {
	Method string          `json:"method"`
	Params json.RawMessage `json:"params"`
}

// Error is the error object in a response.
type Error struct {
	Code    int    `json:"code"`
	Message string `json:"message"`
}

func (e *Error) Error() string { return e.Message }

type response struct {
	Result any    `json:"result,omitempty"`
	Error  *Error `json:"error,omitempty"`
}

// Call runs one method.
func (s *Server) Call(ctx context.Context, method string, params json.RawMessage) (any, error) {
	switch method {
	case MethodGetItem:
		var args struct {
			SKU string `json:"sku"`
		}
		if err := json.Unmarshal(params, &args); err != nil || args.SKU == "" {
			return nil, &Error{Code: 400, Message: "params.sku is required"}
		}
		item, err := s.items.GetItem(ctx, args.SKU)
		if errors.Is(err, store.ItemNotFoundErr) {
			return nil, &Error{Code: 404, Message: "item not found"}
		}
		if err != nil {
			return nil, err
		}
		return item, nil
	case MethodListItems:
		return s.items.GetItems(ctx)
	case MethodCount:
		items, err := s.items.GetItems(ctx)
		if err != nil {
			return nil, err
		}
		return map[string]int{"count": len(items)}, nil
	default:
		return nil, &Error{Code: 404, Message: "unknown method " + method}
	}
}

// ServeHTTP handles POST /rpc.
func (s *Server) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "POST only", http.StatusMethodNotAllowed)
		return
	}
	var req request
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeJSON(w, http.StatusBadRequest, response{Error: &Error{Code: 400, Message: "malformed request"}})
		return
	}
	if len(req.Params) == 0 {
		req.Params = json.RawMessage("{}")
	}
	result, err := s.Call(r.Context(), req.Method, req.Params)
	var rpcErr *Error
	switch {
	case errors.As(err, &rpcErr):
		writeJSON(w, http.StatusOK, response{Error: rpcErr})
	case err != nil:
		writeJSON(w, http.StatusInternalServerError, response{Error: &Error{Code: 500, Message: "internal error"}})
	default:
		writeJSON(w, http.StatusOK, response{Result: result})
	}
}

func writeJSON(w http.ResponseWriter, status int, body response) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(body)
}
