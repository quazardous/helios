kind: added

`library.aggregate` computes the median, any percentile (`p` from 0 to 100) and the median absolute deviation of a stored measure, and splits a set into shooting sessions: a new session starts when two consecutive shots are further apart than a gap (`gap_minutes`, 30 by default). A photo without the measure is still skipped and listed as missing.
