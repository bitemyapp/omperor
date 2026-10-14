// Command checkrename fails if a Go tree still declares or uses the old names
// (GetItem, ItemNotFoundErr) as identifiers or mentions them in comments, or
// if the wire method string "Inventory.GetItem" disappeared.
//
//	go run checkrename.go DIR
package main

import (
	"fmt"
	"go/ast"
	"go/parser"
	"go/token"
	"io/fs"
	"os"
	"path/filepath"
	"regexp"
	"strings"
)

var oldNames = map[string]bool{"GetItem": true, "ItemNotFoundErr": true}

var oldInText = regexp.MustCompile(`\bGetItem\b|\bItemNotFoundErr\b`)

func main() {
	root := os.Args[1]
	fset := token.NewFileSet()
	var problems []string
	wireName := false
	files := 0
	err := filepath.WalkDir(root, func(path string, entry fs.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if entry.IsDir() {
			if name := entry.Name(); name == ".git" || name == "vendor" || name == "testdata" {
				return filepath.SkipDir
			}
			return nil
		}
		if !strings.HasSuffix(path, ".go") {
			return nil
		}
		files++
		file, err := parser.ParseFile(fset, path, nil, parser.ParseComments)
		if err != nil {
			problems = append(problems, fmt.Sprintf("%s: does not parse: %v", path, err))
			return nil
		}
		ast.Inspect(file, func(node ast.Node) bool {
			switch node := node.(type) {
			case *ast.Ident:
				if oldNames[node.Name] {
					problems = append(problems, fmt.Sprintf("%s: identifier %s", fset.Position(node.Pos()), node.Name))
				}
			case *ast.BasicLit:
				if node.Kind == token.STRING && strings.Trim(node.Value, "\"`") == "Inventory.GetItem" {
					wireName = true
				}
			}
			return true
		})
		for _, group := range file.Comments {
			for _, comment := range group.List {
				text := strings.ReplaceAll(comment.Text, "Inventory.GetItem", "")
				if oldInText.MatchString(text) {
					problems = append(problems, fmt.Sprintf("%s: comment still says %q", fset.Position(comment.Pos()), oldInText.FindString(text)))
				}
			}
		}
		return nil
	})
	if err != nil {
		problems = append(problems, err.Error())
	}
	if files == 0 {
		problems = append(problems, "no Go files found")
	}
	if !wireName {
		problems = append(problems, `the wire method string "Inventory.GetItem" is gone`)
	}
	for _, problem := range problems {
		fmt.Println(problem)
	}
	if len(problems) > 0 {
		os.Exit(1)
	}
	fmt.Printf("checked %d Go files\n", files)
}
