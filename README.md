# forwardhook

A simple webhook forwarder

## Usage

```
forwardhook [CONFIG_FILE]
```

## Example

Using the following config

```json5
{
  "port": 8080, // Port to listen on
  "userAgent": "WebhookForwarder/1.0.0", // User-Agent, defaults to "forwardhook/${version}"
  "webhooks": {
    "example": {
      "forwardUrl": "http://example.com", // Url to forward the JSON to
      "forwardMethod": "POST", // HTTP method of the forwarded request, defaults to "POST"
      "fields": [
        {
          "from": ["todos", 0, "description"], // Path of the value to grab in the incoming JSON
          "to": ["description"], // Path where to copy this value in the forwarded JSON
          "optional": false // Whether to skip this field if it's not present, defaults to false
        }
      ]
    }
  },
  "debug": false // Replies with the generated JSON instead of forwarding it, defaults to false
}
```

all valid requests to `/example` would be forwarded to `http://example.com`, for instance

```json
{
  "todos": [
    {
      "name": "Laundry",
      "description": "Do the laundry",
      "done": false
    },
    {
      "name": "Dishes",
      "description": "Clean the dishes",
      "done": true
    }
  ]
}
```

would become

```json
{
  "description": "Do the laundry"
}
```
