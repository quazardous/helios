kind: added

The vault identifies an image by its content, not by where it is: a folder
import records every copy of a file as a location, a file already known is
never read again, and a repair scan finds moved or renamed files by content
while reading as little as it can.
