# timespan

Small helpers (and a CLI) for human-readable durations. Standard library only.

```console
$ python3 -m timespan format 5400
1h30m
$ python3 -m timespan format 788645
1w2d3h4m5s
```

Errors go to stderr as `timespan: error: ...` with exit status 2.

Run the tests with `python3 -m unittest discover -s tests -t .`.
