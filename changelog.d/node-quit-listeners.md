kind: changed

The library's node stops as its own quit request (`helios.node.quit`): the backups, the image map, the health check, the exports, the job mirror and other jobs each stop as a named listener with its own deadline, then the library closes, always last. A listener past its deadline is cut and the quit goes on. The node says which listener runs and what each did as facts: the closing splash shows "Waiting for: exports", and the quit report (`last-quit.json`) names the node's listeners that were cut. An import's cancel is now heard as it is asked, not polled.
