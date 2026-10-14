# slugkit

URL slugs for article titles.

```python
>>> from slugkit import slugify
>>> slugify("Hello, World!")
'hello-world'
>>> slugify("The quick brown fox", max_length=12)
'the-quick'
```

`max_length` never splits a word unless the first word alone is too long.

Run the tests with `python3 -m unittest discover -s tests`.
