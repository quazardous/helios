kind: added

Thumbnails follow their files: in the background, the photos on screen first, then those shown recently, then the Library, Helios checks each cached thumbnail against its file (size and time, then a partial and a full read only when they changed) and makes a new one when the original was edited elsewhere. It waits on battery and while imports, exports, backups or renders run.
