kind: fixed

A photo's full-size step is reused again when its content is already there: a reduced render reads a full-resolution render that is ready or being computed instead of growing past it and computing it again, and small images (a side under 1024 px) are no longer grown by the 64 px whole-image margin.
