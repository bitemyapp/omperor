// Command stockctl inspects the demo inventory or serves the scanner RPC.
//
//	stockctl get SKU        print one item (via GetItem)
//	stockctl low N SKU...   print items at or below N
//	stockctl serve ADDR     serve POST /rpc
package main

import (
	"context"
	"errors"
	"fmt"
	"net/http"
	"os"
	"strconv"
	"time"

	"example.com/stockroom/api"
	"example.com/stockroom/cache"
	"example.com/stockroom/internal/seed"
	"example.com/stockroom/report"
	"example.com/stockroom/store"
)

func main() {
	if err := run(os.Args[1:]); err != nil {
		fmt.Fprintln(os.Stderr, "stockctl:", err)
		os.Exit(1)
	}
}

func run(args []string) error {
	if len(args) == 0 {
		return errors.New("usage: stockctl get SKU | low N SKU... | serve ADDR")
	}
	ctx := context.Background()
	items := cache.New(store.New(seed.Demo()...), time.Minute)
	switch args[0] {
	case "get":
		if len(args) != 2 {
			return errors.New("usage: stockctl get SKU")
		}
		item, err := items.GetItem(ctx, args[1])
		if errors.Is(err, store.ItemNotFoundErr) {
			return fmt.Errorf("no item with SKU %q", args[1])
		}
		if err != nil {
			return err
		}
		fmt.Printf("%s\t%s\t%d\n", item.SKU, item.Name, item.Quantity)
		return nil
	case "low":
		if len(args) < 3 {
			return errors.New("usage: stockctl low N SKU...")
		}
		threshold, err := strconv.Atoi(args[1])
		if err != nil {
			return fmt.Errorf("threshold: %w", err)
		}
		low, err := report.LowStock(ctx, items, args[2:], threshold)
		if err != nil {
			return err
		}
		return report.Write(os.Stdout, low)
	case "serve":
		if len(args) != 2 {
			return errors.New("usage: stockctl serve ADDR")
		}
		mux := http.NewServeMux()
		mux.Handle("/rpc", api.NewServer(items))
		return http.ListenAndServe(args[1], mux)
	default:
		return fmt.Errorf("unknown command %q", args[0])
	}
}
