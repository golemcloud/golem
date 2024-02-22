FROM nginx:latest

COPY /golem-router/golem-services.conf.template /etc/nginx/nginx.conf.template

EXPOSE 80

CMD ["nginx", "-g", "daemon off;"]