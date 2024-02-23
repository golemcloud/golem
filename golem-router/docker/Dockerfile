FROM nginx:latest

COPY /golem-router/golem-services.conf.template /etc/nginx/nginx.conf.template

EXPOSE 80

CMD ["envsubst", "<", "/etc/nginx/nginx.conf.template", ">", "/etc/nginx/nginx.conf", "&&", "nginx", "-g", "daemon off;"]
