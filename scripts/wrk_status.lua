-- wrk: collect HTTP status counts (wrk 4.x response callback)
responses = {}

response = function(status, headers, body)
  responses[status] = (responses[status] or 0) + 1
end

done = function(summary, latency, requests)
  io.write("WRK_STATUS_DIST:")
  for k, v in pairs(responses) do
    io.write(string.format(" %d:%d", k, v))
  end
  io.write("\n")
end
